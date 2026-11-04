//! `runtime = local` launch and stop (ur-y1f6n): the worker runs natively in
//! its pool slot under builderd's `BuilderLocalWorkerService`.
//!
//! The launch shares `run_and_record`'s provisional-row protocol and the
//! container path's project/mode resolution (`resolve_*` and the
//! `RunOptsBuilder` path helpers), but skips the Docker network, Squid proxy
//! env, image and pre-launch container cleanup. Every path sent to builderd is
//! a host path: templates resolve against `host_config_dir`, never `/config`.

use std::path::Path;

use tracing::info;
use uuid::Uuid;

use ur_config::{ResolvedTemplatePath, resolve_template_path};
use ur_rpc::proto::builder_local_worker::{
    EnvVar, HostSkill, LaunchLocalWorkerRequest, LocalMount,
};

use crate::run_opts_builder::{prepare_host_dir, resolve_mount_source};
use crate::worker::{
    WorkerConfig, WorkerId, WorkerManager, context_mount_configs, resolve_brain_dir,
    resolve_memory_dir, resolve_project_instruction, worker_skills,
};

/// Recorded as the row's `container_id`: builderd's tmux session name.
fn local_session_name(worker_id: &WorkerId) -> String {
    format!("ur-{worker_id}")
}

fn host_string(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

impl WorkerManager {
    /// `run_and_record` for `runtime = local`. On launch failure the row is
    /// deleted only once `StopLocalWorker` confirms builderd holds nothing;
    /// otherwise see [`Self::abandon_local_launch`]. builderd's error text is
    /// returned either way. Returns `(session_name, worker_secret)`.
    pub(crate) async fn run_and_record_local(
        &self,
        config: WorkerConfig,
    ) -> Result<(String, String), String> {
        let worker_secret = Uuid::new_v4().to_string();
        let session = local_session_name(&config.worker_id);
        let request = build_local_launch_request(
            &config,
            &worker_secret,
            &self.host_config_dir,
            self.worker_port,
        )?;

        // The agent's SessionStart hook authenticates right away.
        self.record_provisioning_worker(&config, &session, &worker_secret)
            .await?;

        if let Err(status) = self
            .builder_local_worker_client
            .launch_local_worker(request)
            .await
        {
            let error = format!("launch_local_worker gRPC error: {status}");
            return Err(self.abandon_local_launch(&config, error).await);
        }
        info!(
            process_id = config.process_id,
            worker_id = %config.worker_id,
            session,
            "local worker launched"
        );

        if let Err(error) = self.finalize_worker_launch(&config, &session).await {
            return Err(self.abandon_local_launch(&config, error).await);
        }
        Ok((session, worker_secret))
    }

    /// Clean up after a failed launch. A failed launch may still have left a
    /// live session (e.g. its teardown failed), so the slot is let go only
    /// once `StopLocalWorker` succeeds; otherwise the row becomes `stopping`
    /// with the slot linked to it, and the reconcile loop retries the stop.
    async fn abandon_local_launch(&self, config: &WorkerConfig, error: String) -> String {
        let worker_id = &config.worker_id;
        let Err(status) = self
            .builder_local_worker_client
            .stop_local_worker(&worker_id.0)
            .await
        else {
            return self.remove_provisioning_worker(worker_id, error).await;
        };
        let mut error = format!("{error}; failed to clean up local worker: {status}");
        if let Err(e) = self.hold_slot_for_retry(config).await {
            error.push_str(&format!("; {e}"));
        }
        error
    }

    /// Keep a failed launch's slot out of the pool: link it to the worker and
    /// mark the row `stopping` so the reconcile loop retries the stop.
    async fn hold_slot_for_retry(&self, config: &WorkerConfig) -> Result<(), String> {
        let worker_id = &config.worker_id.0;
        if let Some(slot_id) = &config.slot_id
            && self
                .worker_repo
                .get_worker_slot(worker_id)
                .await
                .map_err(|e| format!("failed to read slot link: {e}"))?
                .is_none()
        {
            self.worker_repo
                .link_worker_slot(worker_id, slot_id)
                .await
                .map_err(|e| format!("failed to link slot for retry: {e}"))?;
        }
        self.worker_repo
            .begin_stopping(worker_id)
            .await
            .map_err(|e| format!("failed to mark worker stopping: {e}"))?;
        Ok(())
    }

    /// Mark the row `stopping` (only from an active status), then tear the
    /// worker down via builderd. `Ok(false)`: the row is already stopped or
    /// gone, so there is nothing to stop or release. On failure the row
    /// stays `stopping` and builderd's error text is returned.
    pub(crate) async fn stop_local_worker(&self, worker_id: &WorkerId) -> Result<bool, String> {
        if !self
            .worker_repo
            .begin_stopping(&worker_id.0)
            .await
            .map_err(|e| format!("failed to update worker status: {e}"))?
        {
            return Ok(false);
        }
        self.builder_local_worker_client
            .stop_local_worker(&worker_id.0)
            .await
            .map_err(|status| format!("stop_local_worker gRPC error: {status}"))?;
        Ok(true)
    }
}

/// Build builderd's launch request from the same resolution the container
/// path uses, with host paths throughout. Memory and brain dirs are created
/// (not chowned: the agent runs as the host user).
pub(crate) fn build_local_launch_request(
    config: &WorkerConfig,
    worker_secret: &str,
    host_config_dir: &Path,
    worker_port: u16,
) -> Result<LaunchLocalWorkerRequest, String> {
    let agent = ur_config::AgentType::parse(&config.agent_type).map_err(|e| e.to_string())?;
    let slot_path = config
        .workspace_dir
        .as_deref()
        .ok_or("a local worker needs a pool slot")?;

    let context_mounts = context_mount_configs(&config.context_mounts);
    let mounts = config
        .mounts
        .iter()
        .chain(&context_mounts)
        .map(|mount| {
            Ok(LocalMount {
                source: host_string(&resolve_mount_source(&mount.source, host_config_dir)?),
                dest: mount.destination.clone(),
                read_only: mount.readonly,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;

    let host_dir = |template: Option<String>, key: &str| -> Result<String, String> {
        template
            .map(|t| prepare_host_dir(&t, host_config_dir, key, None).map(|p| host_string(&p)))
            .transpose()
            .map(Option::unwrap_or_default)
    };
    let memory_dir = host_dir(
        resolve_memory_dir(&config.memory_dir, &config.project_key, host_config_dir),
        "memory_dir",
    )?;
    let brain_dir = host_dir(
        resolve_brain_dir(
            &config.brain_dir,
            &config.workspace_brain_dir,
            &config.project_key,
            host_config_dir,
        ),
        "brain_dir",
    )?;

    let project_instruction_path = match resolve_project_instruction(
        &config.instruction_md,
        &config.project_key,
        host_config_dir,
        agent,
    ) {
        None => String::new(),
        Some(template) => match resolve_template_path(&template, host_config_dir)
            .map_err(|e| format!("failed to resolve instruction_md: {e}"))?
        {
            ResolvedTemplatePath::HostPath(path) => host_string(&path),
            ResolvedTemplatePath::ProjectRelative(rel) => host_string(&slot_path.join(rel)),
        },
    };

    Ok(LaunchLocalWorkerRequest {
        worker_id: config.worker_id.0.clone(),
        slot_path: host_string(slot_path),
        agent_type: agent.name().to_owned(),
        model: config.model.clone(),
        effort: config.effort.clone(),
        env: local_env(config, worker_secret, worker_port),
        mounts,
        memory_dir,
        brain_dir,
        skills: worker_skills(config),
        instruction_strategy: config.strategy.instruction_strategy_name().to_owned(),
        project_key: config.project_key.clone(),
        project_instruction_path,
        allow_no_verify: config.allow_no_verify,
        host_skills: config
            .extra_skill_mounts
            .iter()
            .map(|(name, path)| HostSkill {
                name: name.clone(),
                source: host_string(path),
            })
            .collect(),
    })
}

/// Session env the server supplies. The worker server is published on host
/// loopback; builderd adds its own identity and `UR_WORKERD_ADDR`.
fn local_env(config: &WorkerConfig, worker_secret: &str, worker_port: u16) -> Vec<EnvVar> {
    let mut env = vec![
        (
            ur_config::UR_SERVER_ADDR_ENV,
            format!("127.0.0.1:{worker_port}"),
        ),
        (ur_config::UR_WORKER_SECRET_ENV, worker_secret.to_owned()),
    ];
    if !config.project_key.is_empty() {
        env.push(("UR_PROJECT", config.project_key.clone()));
    }
    if let Some(forge) = config.forge {
        env.push((ur_config::UR_WORKER_FORGE_ENV, forge.as_str().to_owned()));
    }
    env.into_iter()
        .map(|(key, value)| EnvVar {
            key: key.to_owned(),
            value,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::WorkerStrategy;

    fn config(slot: &Path) -> WorkerConfig {
        WorkerConfig {
            process_id: "proj-1".into(),
            worker_id: WorkerId("proj-1-abcd".into()),
            image_id: String::new(),
            cpus: 0,
            memory: String::new(),
            workspace_dir: Some(slot.to_path_buf()),
            proxy_hostname: "ur-squid".into(),
            project_key: "proj".into(),
            agent_type: "claude".into(),
            strategy: WorkerStrategy::Code,
            skills: Vec::new(),
            model: "opus".into(),
            effort: "high".into(),
            instruction_md: None,
            mounts: Vec::new(),
            ports: Vec::new(),
            slot_id: None,
            context_mounts: Vec::new(),
            hostexec_scripts: Vec::new(),
            extra_skill_mounts: Vec::new(),
            memory_dir: None,
            brain_dir: None,
            workspace_brain_dir: None,
            forge: None,
            runtime: ur_config::WorkerRuntime::Local,
            allow_no_verify: true,
        }
    }

    struct Dirs {
        _tmp: tempfile::TempDir,
        host_config: PathBuf,
        slot: PathBuf,
    }

    fn dirs() -> Dirs {
        let tmp = tempfile::tempdir().unwrap();
        let host_config = tmp.path().join("host-ur");
        let slot = tmp.path().join("pool/proj/0");
        std::fs::create_dir_all(&slot).unwrap();
        Dirs {
            host_config,
            slot,
            _tmp: tmp,
        }
    }

    fn build(config: &WorkerConfig, d: &Dirs) -> LaunchLocalWorkerRequest {
        build_local_launch_request(config, "s3cret", &d.host_config, 12322).unwrap()
    }

    fn mount(source: &str, destination: &str, readonly: bool) -> ur_config::MountConfig {
        ur_config::MountConfig {
            source: source.into(),
            destination: destination.into(),
            readonly,
        }
    }

    #[test]
    fn mounts_and_context_repos_use_host_sources() {
        let d = dirs();
        let cfg = WorkerConfig {
            mounts: vec![
                mount("%URCONFIG%/shared/notes", "/notes", true),
                mount("/abs/data", "/var/data", false),
            ],
            context_mounts: vec![("other".into(), PathBuf::from("/pool/other/shared"))],
            ..config(&d.slot)
        };
        let req = build(&cfg, &d);
        let got: Vec<_> = req
            .mounts
            .iter()
            .map(|m| (m.source.as_str(), m.dest.as_str(), m.read_only))
            .collect();
        let notes = host_string(&d.host_config.join("shared/notes"));
        assert_eq!(
            got,
            vec![
                (notes.as_str(), "/notes", true),
                ("/abs/data", "/var/data", false),
                ("/pool/other/shared", "/context/other", false),
            ]
        );
        assert_eq!(req.slot_path, host_string(&d.slot));
    }

    #[test]
    fn project_relative_mount_is_rejected() {
        let d = dirs();
        let cfg = WorkerConfig {
            mounts: vec![mount("%PROJECT%/x", "/x", false)],
            ..config(&d.slot)
        };
        let err = build_local_launch_request(&cfg, "s", &d.host_config, 1).unwrap_err();
        assert!(err.contains("project-relative"), "{err}");
    }

    #[test]
    fn skills_add_gitea_and_host_skills_carry_sources() {
        let d = dirs();
        let cfg = WorkerConfig {
            skills: vec!["implement".into(), "team".into()],
            extra_skill_mounts: vec![("team".into(), PathBuf::from("/host/skills/team"))],
            forge: Some(ur_config::ForgeType::Gitea),
            ..config(&d.slot)
        };
        let req = build(&cfg, &d);
        assert_eq!(req.skills, vec!["implement", "team", "gitea"]);
        assert_eq!(
            req.host_skills,
            vec![HostSkill {
                name: "team".into(),
                source: "/host/skills/team".into(),
            }]
        );
    }

    #[test]
    fn memory_and_brain_templates_resolve_to_created_host_dirs() {
        let d = dirs();
        let cfg = WorkerConfig {
            memory_dir: Some("%URCONFIG%/mem/proj".into()),
            brain_dir: Some("%URCONFIG%/brains/proj".into()),
            ..config(&d.slot)
        };
        let req = build(&cfg, &d);
        for (got, rel) in [
            (&req.memory_dir, "mem/proj"),
            (&req.brain_dir, "brains/proj"),
        ] {
            let want = d.host_config.join(rel);
            assert_eq!(*got, host_string(&want));
            assert!(want.is_dir(), "{rel} created");
        }
    }

    #[test]
    fn memory_and_brain_fall_back_to_project_convention_dirs() {
        let d = dirs();
        let empty = build(&config(&d.slot), &d);
        assert_eq!(
            (empty.memory_dir.as_str(), empty.brain_dir.as_str()),
            ("", "")
        );

        let project = d.host_config.join("projects/proj");
        for sub in ["memory", "brain"] {
            std::fs::create_dir_all(project.join(sub)).unwrap();
        }
        let req = build(&config(&d.slot), &d);
        assert_eq!(req.memory_dir, host_string(&project.join("memory")));
        assert_eq!(req.brain_dir, host_string(&project.join("brain")));
    }

    #[test]
    fn no_project_uses_workspace_brain() {
        let d = dirs();
        let cfg = WorkerConfig {
            project_key: String::new(),
            workspace_brain_dir: Some("%URCONFIG%/ws-brain".into()),
            ..config(&d.slot)
        };
        let req = build(&cfg, &d);
        assert_eq!(req.brain_dir, host_string(&d.host_config.join("ws-brain")));
        assert!(req.env.iter().all(|e| e.key != "UR_PROJECT"));
    }

    #[test]
    fn instruction_paths_are_host_paths() {
        let d = dirs();
        let convention = d.host_config.join("projects/proj/CLAUDE.md");
        std::fs::create_dir_all(convention.parent().unwrap()).unwrap();
        std::fs::write(&convention, "x").unwrap();
        let req = build(&config(&d.slot), &d);
        assert_eq!(req.project_instruction_path, host_string(&convention));

        let cfg = WorkerConfig {
            instruction_md: Some("%PROJECT%/docs/AGENTS.md".into()),
            ..config(&d.slot)
        };
        let req = build(&cfg, &d);
        assert_eq!(
            req.project_instruction_path,
            host_string(&d.slot.join("docs/AGENTS.md"))
        );
    }

    #[test]
    fn scalar_fields_and_env() {
        let d = dirs();
        let cfg = WorkerConfig {
            forge: Some(ur_config::ForgeType::GitHub),
            ..config(&d.slot)
        };
        let req = build(&cfg, &d);
        assert_eq!(req.worker_id, "proj-1-abcd");
        assert_eq!(req.agent_type, "claude");
        assert_eq!((req.model.as_str(), req.effort.as_str()), ("opus", "high"));
        assert_eq!(req.instruction_strategy, "code");
        assert_eq!(req.project_key, "proj");
        assert!(req.allow_no_verify);
        let env: Vec<_> = req
            .env
            .iter()
            .map(|e| (e.key.as_str(), e.value.as_str()))
            .collect();
        assert_eq!(
            env,
            vec![
                (ur_config::UR_SERVER_ADDR_ENV, "127.0.0.1:12322"),
                (ur_config::UR_WORKER_SECRET_ENV, "s3cret"),
                ("UR_PROJECT", "proj"),
                (ur_config::UR_WORKER_FORGE_ENV, "github"),
            ]
        );
    }

    #[test]
    fn missing_slot_is_an_error() {
        let d = dirs();
        let cfg = WorkerConfig {
            workspace_dir: None,
            ..config(&d.slot)
        };
        let err = build_local_launch_request(&cfg, "s", &d.host_config, 1).unwrap_err();
        assert!(err.contains("pool slot"), "{err}");
    }
}
