//! Build and tear down a local (host-side) worker's drive.
//!
//! Layout (see epic ur-wdfir):
//!
//! ```text
//! <workspace_root>/drive/<worker_id>/
//!   <mount dest…> -> <mount source>
//!   brain -> <brain_dir>
//!   .ur/{settings.json, instructions.md, plugin/, skill-hooks/}
//! <home>/.claude/projects/<encoded slot>/memory -> <memory_dir>
//! ```
//!
//! Data safety: teardown only ever removes links and the drive itself
//! (`remove_dir_all` does not follow symlinks), never a link target.

use std::path::{Component, Path, PathBuf};

use ur_config::AgentType;
use workerd::{
    InitGitHooksManager, InitInstructionsManager, InitSkillHooksManager, InitSkillsManager,
    InstructionInputs, ProjectInstruction, ProjectInstructionMode, SkillLayout, WorkerPaths,
};

use crate::local_settings::{DriveLink, LocalSettingsManager};

const DRIVE_DIR: &str = "drive";
const UR_DIR: &str = ".ur";
const BRAIN_LINK: &str = "brain";

/// A `container.mounts` entry mapped onto the drive.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalMount {
    pub source: PathBuf,
    /// Container-style destination; the leading `/` is stripped.
    pub dest: String,
    /// Advisory only for local workers.
    pub read_only: bool,
}

/// Inputs for [`LocalWorkspaceManager::build`], mirroring the epic's
/// `LaunchLocalWorkerRequest`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalWorkspaceRequest {
    pub worker_id: String,
    pub slot_path: PathBuf,
    pub agent: AgentType,
    pub model: Option<String>,
    pub effort: String,
    pub mounts: Vec<LocalMount>,
    pub memory_dir: Option<PathBuf>,
    pub brain_dir: Option<PathBuf>,
    pub skills: Vec<String>,
    pub instruction_strategy: Option<String>,
    pub project_key: String,
    pub project_instruction_path: Option<PathBuf>,
    pub allow_no_verify: bool,
}

/// A built drive and the agent command to launch in the slot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalWorkspace {
    pub drive: PathBuf,
    /// Memory symlink created (or confirmed) for this slot, if any.
    pub memory_link: Option<PathBuf>,
    pub argv: Vec<String>,
}

#[derive(Debug)]
pub enum LocalWorkspaceError {
    /// The request cannot be satisfied in the current host state.
    FailedPrecondition(String),
    Io {
        context: String,
        source: std::io::Error,
    },
}

impl std::fmt::Display for LocalWorkspaceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::FailedPrecondition(msg) => f.write_str(msg),
            Self::Io { context, source } => write!(f, "{context}: {source}"),
        }
    }
}

impl std::error::Error for LocalWorkspaceError {}

impl From<LocalWorkspaceError> for tonic::Status {
    fn from(err: LocalWorkspaceError) -> Self {
        match err {
            LocalWorkspaceError::FailedPrecondition(_) => {
                tonic::Status::failed_precondition(err.to_string())
            }
            LocalWorkspaceError::Io { .. } => tonic::Status::internal(err.to_string()),
        }
    }
}

type Result<T> = std::result::Result<T, LocalWorkspaceError>;

fn io(context: impl Into<String>) -> impl FnOnce(std::io::Error) -> LocalWorkspaceError {
    let context = context.into();
    move |source| LocalWorkspaceError::Io { context, source }
}

fn precondition(msg: String) -> LocalWorkspaceError {
    LocalWorkspaceError::FailedPrecondition(msg)
}

/// Builds and tears down local worker drives.
#[derive(Clone)]
pub struct LocalWorkspaceManager {
    /// Root config dir (`~/.ur`); host hook overlays live under `projects/<key>/hooks`.
    config_dir: PathBuf,
    /// Workspace root (`~/.ur/workspace`); drives live under `drive/`.
    workspace_root: PathBuf,
    /// The user's home, for agent memory dirs.
    home: PathBuf,
    /// Asset root with `potential-skills/`, `instructions/`, `shared-instructions/`.
    agent_shared: PathBuf,
    settings: LocalSettingsManager,
}

impl LocalWorkspaceManager {
    pub fn new(
        config_dir: PathBuf,
        workspace_root: PathBuf,
        home: PathBuf,
        agent_shared: PathBuf,
        settings: LocalSettingsManager,
    ) -> Self {
        LocalWorkspaceManager {
            config_dir,
            workspace_root,
            home,
            agent_shared,
            settings,
        }
    }

    pub fn drive_path(&self, worker_id: &str) -> PathBuf {
        self.workspace_root.join(DRIVE_DIR).join(worker_id)
    }

    /// Build the drive for `req`. On failure, everything built so far is torn
    /// down before the original error is returned.
    pub async fn build(&self, req: &LocalWorkspaceRequest) -> Result<LocalWorkspace> {
        if !req.agent.supports_local_runtime() {
            return Err(precondition(format!(
                "agent '{}' does not support the local runtime",
                req.agent.name()
            )));
        }
        validate_worker_id(&req.worker_id)?;
        match self.build_inner(req).await {
            Ok(built) => Ok(built),
            Err(err) => {
                // Best effort: the build error is the one the caller needs.
                let _ = self
                    .teardown(&req.worker_id, &req.slot_path, req.agent)
                    .await;
                Err(err)
            }
        }
    }

    async fn build_inner(&self, req: &LocalWorkspaceRequest) -> Result<LocalWorkspace> {
        self.check_assets(req)?;
        let drive = self.drive_path(&req.worker_id);
        // A stale drive from a crashed run is ours to replace.
        remove_tree_if_present(&drive).await?;
        let ur_dir = drive.join(UR_DIR);
        tokio::fs::create_dir_all(&ur_dir)
            .await
            .map_err(io(format!("create {}", ur_dir.display())))?;

        let links = self.link_mounts(&drive, req).await?;
        let memory_link = self.link_memory(req).await?;
        let paths = self.worker_paths(&drive, req);
        self.write_customization(&drive, &paths, req, &links)
            .await?;

        Ok(LocalWorkspace {
            argv: self.agent_argv(&paths, &drive, req, &links),
            drive,
            memory_link,
        })
    }

    /// Fail before any filesystem work if a requested asset is missing; the
    /// shared init managers only warn about these.
    fn check_assets(&self, req: &LocalWorkspaceRequest) -> Result<()> {
        if let Some(strategy) = &req.instruction_strategy {
            let path = self
                .agent_shared
                .join("instructions")
                .join(format!("{strategy}.md"));
            if !path.exists() {
                return Err(precondition(format!(
                    "instruction strategy not found: {}",
                    path.display()
                )));
            }
        }
        for skill in &req.skills {
            let path = self.agent_shared.join("potential-skills").join(skill);
            if !path.exists() {
                return Err(precondition(format!("skill not found: {}", path.display())));
            }
        }
        Ok(())
    }

    /// `WorkerPaths` rooted at the slot (workspace) and the drive's `.ur/`.
    fn worker_paths(&self, drive: &Path, req: &LocalWorkspaceRequest) -> WorkerPaths {
        let ur_dir = drive.join(UR_DIR);
        WorkerPaths {
            workspace: req.slot_path.clone(),
            skill_hooks: ur_dir.join("skill-hooks"),
            skills_dest: ur_dir.join("plugin"),
            instructions_dest: ur_dir.join("instructions.md"),
            settings_template: None,
            settings_dest: Some(ur_dir.join("settings.json")),
            host_hooks: self
                .config_dir
                .join("projects")
                .join(&req.project_key)
                .join("hooks"),
            agent_shared: self.agent_shared.clone(),
            shim_dir: ur_dir.join("bin"),
        }
    }

    /// Link every mount and the brain into the drive. Returns the links in
    /// request order (mounts, then brain).
    async fn link_mounts(
        &self,
        drive: &Path,
        req: &LocalWorkspaceRequest,
    ) -> Result<Vec<DriveLink>> {
        let mut links = Vec::new();
        for mount in &req.mounts {
            let rel = Path::new(mount.dest.trim_start_matches('/'));
            if rel.as_os_str().is_empty()
                || !rel.components().all(|c| matches!(c, Component::Normal(_)))
            {
                return Err(precondition(format!(
                    "invalid mount destination '{}'",
                    mount.dest
                )));
            }
            let link = drive.join(rel);
            create_link(drive, &mount.source, &link).await?;
            links.push(DriveLink {
                link,
                source: mount.source.clone(),
                read_only: mount.read_only,
            });
        }
        if let Some(brain) = &req.brain_dir {
            let link = drive.join(BRAIN_LINK);
            create_link(drive, brain, &link).await?;
            links.push(DriveLink {
                link,
                source: brain.clone(),
                read_only: false,
            });
        }
        Ok(links)
    }

    /// Point the agent's memory dir for the slot at `memory_dir`. Never
    /// clobbers a real non-empty directory or a file.
    async fn link_memory(&self, req: &LocalWorkspaceRequest) -> Result<Option<PathBuf>> {
        let (Some(target), Some(link)) = (
            &req.memory_dir,
            req.agent.local_memory_dir(&self.home, &req.slot_path),
        ) else {
            return Ok(None);
        };
        let ctx = format!("link memory {}", link.display());
        match tokio::fs::symlink_metadata(&link).await {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                if let Some(parent) = link.parent() {
                    tokio::fs::create_dir_all(parent).await.map_err(io(&ctx))?;
                }
            }
            Err(e) => return Err(io(ctx)(e)),
            Ok(meta) if meta.is_symlink() => {
                let current = tokio::fs::read_link(&link).await.map_err(io(&ctx))?;
                if &current == target {
                    return Ok(Some(link));
                }
                tokio::fs::remove_file(&link).await.map_err(io(&ctx))?;
            }
            Ok(meta) if meta.is_dir() => {
                let mut entries = tokio::fs::read_dir(&link).await.map_err(io(&ctx))?;
                if entries.next_entry().await.map_err(io(&ctx))?.is_some() {
                    return Err(precondition(format!(
                        "memory dir {} is a non-empty directory; move it aside to link {}",
                        link.display(),
                        target.display()
                    )));
                }
                tokio::fs::remove_dir(&link).await.map_err(io(&ctx))?;
            }
            Ok(_) => {
                return Err(precondition(format!(
                    "memory dir {} exists and is not a directory",
                    link.display()
                )));
            }
        }
        tokio::fs::symlink(target, &link).await.map_err(io(ctx))?;
        Ok(Some(link))
    }

    /// Plugin skills, skill hooks, git hooks, settings.json and instructions.md.
    async fn write_customization(
        &self,
        drive: &Path,
        paths: &WorkerPaths,
        req: &LocalWorkspaceRequest,
        links: &[DriveLink],
    ) -> Result<()> {
        let layout = SkillLayout::ur_plugin();
        InitSkillsManager::new(paths.clone(), layout.clone())
            .run(&req.skills)
            .await
            .map_err(io("install ur plugin skills"))?;
        InitSkillHooksManager::new(paths.clone())
            .run()
            .await
            .map_err(io("install skill hooks"))?;
        InitGitHooksManager::new(paths.clone())
            .run()
            .await
            .map_err(io("install git hooks into slot"))?;

        let settings_path = drive.join(UR_DIR).join("settings.json");
        let settings = self.settings.settings_json(req.allow_no_verify);
        let body = serde_json::to_string_pretty(&settings)
            .map_err(|e| io("serialize settings.json")(std::io::Error::other(e)))?;
        tokio::fs::write(&settings_path, body)
            .await
            .map_err(io(format!("write {}", settings_path.display())))?;

        // Project instructions are inlined: `--append-system-prompt-file`
        // does not expand `@` imports.
        let inputs = InstructionInputs {
            strategy: req.instruction_strategy.clone(),
            forge: None,
            project: req
                .project_instruction_path
                .as_ref()
                .map(|path| ProjectInstruction {
                    path: path.clone(),
                    host_workspace: Some(req.slot_path.to_string_lossy().into_owned()),
                    mode: ProjectInstructionMode::Inline,
                }),
        };
        InitInstructionsManager::new(paths.clone(), layout)
            .run(&inputs)
            .await
            .map_err(io("compose instructions"))?;

        let dest = &paths.instructions_dest;
        let mut content = match tokio::fs::read_to_string(dest).await {
            Ok(text) => text + "\n\n",
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(e) => return Err(io(format!("read {}", dest.display()))(e)),
        };
        content.push_str(&self.settings.instructions_appendix(drive, links));
        tokio::fs::write(dest, content)
            .await
            .map_err(io(format!("write {}", dest.display())))
    }

    fn agent_argv(
        &self,
        paths: &WorkerPaths,
        drive: &Path,
        req: &LocalWorkspaceRequest,
        links: &[DriveLink],
    ) -> Vec<String> {
        let ur_dir = drive.join(UR_DIR);
        let display = |p: &Path| p.to_string_lossy().into_owned();
        let mut argv = vec![
            "claude".to_owned(),
            "--settings".to_owned(),
            display(&ur_dir.join("settings.json")),
            "--append-system-prompt-file".to_owned(),
            display(&paths.instructions_dest),
            "--plugin-dir".to_owned(),
            display(&paths.skills_dest),
            "--add-dir".to_owned(),
            display(drive),
        ];
        for link in links {
            argv.push("--add-dir".to_owned());
            argv.push(display(&link.source));
        }
        if let Some(model) = req.model.as_deref().map(str::trim)
            && !model.is_empty()
        {
            argv.push("--model".to_owned());
            argv.push(model.to_owned());
        }
        let effort = match req.effort.trim() {
            "" => req.agent.default_effort(),
            effort => effort,
        };
        argv.push("--effort".to_owned());
        argv.push(effort.to_owned());
        argv
    }

    /// Remove the slot's memory symlink (only if it is a symlink) and the
    /// drive. Idempotent; every step is attempted and the first error returned.
    pub async fn teardown(
        &self,
        worker_id: &str,
        slot_path: &Path,
        agent: AgentType,
    ) -> Result<()> {
        validate_worker_id(worker_id)?;
        let mut first_err = None;
        if let Some(link) = agent.local_memory_dir(&self.home, slot_path)
            && let Err(e) = remove_symlink_if_present(&link).await
        {
            first_err = Some(e);
        }
        if let Err(e) = remove_tree_if_present(&self.drive_path(worker_id)).await {
            first_err.get_or_insert(e);
        }
        first_err.map_or(Ok(()), Err)
    }
}

/// A worker id must be a single normal path component so the drive path
/// stays inside `drive/`.
fn validate_worker_id(worker_id: &str) -> Result<()> {
    let mut components = Path::new(worker_id).components();
    match (components.next(), components.next()) {
        (Some(Component::Normal(_)), None) => Ok(()),
        _ => Err(precondition(format!("invalid worker id '{worker_id}'"))),
    }
}

/// Symlink `link` -> `source`, creating intermediate directories inside
/// `drive`. Refuses to create directories through an existing symlink, which
/// would write into another mount's source.
async fn create_link(drive: &Path, source: &Path, link: &Path) -> Result<()> {
    if !tokio::fs::try_exists(source)
        .await
        .map_err(io(format!("stat {}", source.display())))?
    {
        return Err(precondition(format!(
            "mount source does not exist: {}",
            source.display()
        )));
    }
    let parent = link.parent().unwrap_or(drive);
    let mut dir = drive.to_path_buf();
    for component in parent.strip_prefix(drive).unwrap_or(Path::new("")) {
        dir.push(component);
        match tokio::fs::symlink_metadata(&dir).await {
            Ok(meta) if meta.is_symlink() => {
                return Err(precondition(format!(
                    "mount {} would nest inside linked mount {}",
                    link.display(),
                    dir.display()
                )));
            }
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                tokio::fs::create_dir(&dir)
                    .await
                    .map_err(io(format!("create {}", dir.display())))?;
            }
            Err(e) => return Err(io(format!("stat {}", dir.display()))(e)),
        }
    }
    tokio::fs::symlink(source, link).await.map_err(io(format!(
        "link {} -> {}",
        link.display(),
        source.display()
    )))
}

async fn remove_symlink_if_present(link: &Path) -> Result<()> {
    let ctx = format!("remove {}", link.display());
    match tokio::fs::symlink_metadata(link).await {
        Ok(meta) if meta.is_symlink() => tokio::fs::remove_file(link).await.map_err(io(ctx)),
        Ok(_) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(io(ctx)(e)),
    }
}

/// `remove_dir_all` (which never follows symlinks); missing is a no-op.
async fn remove_tree_if_present(path: &Path) -> Result<()> {
    match tokio::fs::remove_dir_all(path).await {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(io(format!("remove {}", path.display()))(e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    struct Fixture {
        tmp: TempDir,
        manager: LocalWorkspaceManager,
    }

    impl Fixture {
        fn new() -> Self {
            let tmp = TempDir::new().unwrap();
            let root = tmp.path();
            let write = |rel: &str, body: &str| {
                let p = root.join(rel);
                fs::create_dir_all(p.parent().unwrap()).unwrap();
                fs::write(p, body).unwrap();
            };
            write(
                "shared/potential-skills/implement/SKILL.md",
                "hooks: %UR_SKILL_HOOKS%/implement",
            );
            write("shared/instructions/code.md", "# Code Worker");
            write("shared/shared-instructions/tickets.md", "# Tickets");
            write("slot/ur-hooks/git/pre-commit", "#!/bin/sh\n");
            write("slot/ur-hooks/skills/implement/pre.md", "in-repo");
            write("config/projects/p/hooks/skills/implement/pre.md", "overlay");
            write("slot/PROJECT.md", "Project at %WORKSPACE%");
            write("src/ctx/keep.txt", "ctx");
            write("src/brain/notes.md", "brain");
            write("src/memory/MEMORY.md", "mem");
            fs::create_dir_all(root.join("slot/.git/hooks")).unwrap();
            fs::create_dir_all(root.join("home")).unwrap();
            let manager = LocalWorkspaceManager::new(
                root.join("config"),
                root.join("workspace"),
                root.join("home"),
                root.join("shared"),
                LocalSettingsManager,
            );
            Fixture { tmp, manager }
        }

        fn path(&self, rel: &str) -> PathBuf {
            self.tmp.path().join(rel)
        }

        fn request(&self) -> LocalWorkspaceRequest {
            LocalWorkspaceRequest {
                worker_id: "w1".into(),
                slot_path: self.path("slot"),
                agent: AgentType::Claude,
                model: None,
                effort: String::new(),
                mounts: Vec::new(),
                memory_dir: None,
                brain_dir: None,
                skills: Vec::new(),
                instruction_strategy: None,
                project_key: "p".into(),
                project_instruction_path: None,
                allow_no_verify: false,
            }
        }

        fn memory_link(&self) -> PathBuf {
            AgentType::Claude
                .local_memory_dir(&self.path("home"), &self.path("slot"))
                .unwrap()
        }

        fn with_memory(&self) -> LocalWorkspaceRequest {
            LocalWorkspaceRequest {
                memory_dir: Some(self.path("src/memory")),
                ..self.request()
            }
        }
    }

    fn mount(source: PathBuf, dest: &str, read_only: bool) -> LocalMount {
        LocalMount {
            source,
            dest: dest.into(),
            read_only,
        }
    }

    fn precondition_msg(err: LocalWorkspaceError) -> String {
        match err {
            LocalWorkspaceError::FailedPrecondition(msg) => msg,
            other => panic!("expected FailedPrecondition, got {other}"),
        }
    }

    #[tokio::test]
    async fn links_mounts_nested_dests_and_brain() {
        let f = Fixture::new();
        let req = LocalWorkspaceRequest {
            mounts: vec![
                mount(f.path("src/ctx"), "/ctx", true),
                mount(f.path("src/memory"), "/deep/nested/mem", false),
            ],
            brain_dir: Some(f.path("src/brain")),
            ..f.request()
        };
        let built = f.manager.build(&req).await.unwrap();
        let drive = f.path("workspace/drive/w1");
        assert_eq!(built.drive, drive);
        assert_eq!(fs::read_link(drive.join("ctx")).unwrap(), f.path("src/ctx"));
        assert_eq!(
            fs::read_link(drive.join("deep/nested/mem")).unwrap(),
            f.path("src/memory")
        );
        assert!(
            !fs::symlink_metadata(drive.join("deep"))
                .unwrap()
                .is_symlink()
        );
        assert_eq!(
            fs::read_link(drive.join("brain")).unwrap(),
            f.path("src/brain")
        );
    }

    #[tokio::test]
    async fn missing_mount_source_fails_and_cleans_up() {
        let f = Fixture::new();
        let req = LocalWorkspaceRequest {
            mounts: vec![mount(f.path("src/nope"), "/x", false)],
            ..f.request()
        };
        let msg = precondition_msg(f.manager.build(&req).await.unwrap_err());
        assert!(msg.contains("src/nope"), "{msg}");
        assert!(!f.path("workspace/drive/w1").exists());
    }

    #[tokio::test]
    async fn refuses_dest_nested_inside_another_mount() {
        let f = Fixture::new();
        let req = LocalWorkspaceRequest {
            mounts: vec![
                mount(f.path("src/ctx"), "/ctx", false),
                mount(f.path("src/brain"), "/ctx/sub/b", false),
            ],
            ..f.request()
        };
        let msg = precondition_msg(f.manager.build(&req).await.unwrap_err());
        assert!(msg.contains("nest"), "{msg}");
        assert!(!f.path("src/ctx/sub").exists());
        assert!(f.path("src/ctx/keep.txt").exists());
    }

    #[tokio::test]
    async fn rejects_invalid_dest_and_unsupported_agent() {
        let f = Fixture::new();
        let req = LocalWorkspaceRequest {
            mounts: vec![mount(f.path("src/ctx"), "/../escape", false)],
            ..f.request()
        };
        precondition_msg(f.manager.build(&req).await.unwrap_err());
        let req = LocalWorkspaceRequest {
            agent: AgentType::Codex,
            ..f.request()
        };
        let msg = precondition_msg(f.manager.build(&req).await.unwrap_err());
        assert!(msg.contains("codex"), "{msg}");
    }

    #[tokio::test]
    async fn missing_skill_or_strategy_fails() {
        let f = Fixture::new();
        let req = LocalWorkspaceRequest {
            skills: vec!["ghost".into()],
            ..f.request()
        };
        assert!(precondition_msg(f.manager.build(&req).await.unwrap_err()).contains("ghost"));
        let req = LocalWorkspaceRequest {
            instruction_strategy: Some("ghost".into()),
            ..f.request()
        };
        assert!(precondition_msg(f.manager.build(&req).await.unwrap_err()).contains("ghost"));
    }

    #[tokio::test]
    async fn memory_absent_creates_link() {
        let f = Fixture::new();
        let built = f.manager.build(&f.with_memory()).await.unwrap();
        assert_eq!(built.memory_link, Some(f.memory_link()));
        assert_eq!(
            fs::read_link(f.memory_link()).unwrap(),
            f.path("src/memory")
        );
    }

    #[tokio::test]
    async fn memory_unset_creates_nothing() {
        let f = Fixture::new();
        let built = f.manager.build(&f.request()).await.unwrap();
        assert_eq!(built.memory_link, None);
        assert!(fs::symlink_metadata(f.memory_link()).is_err());
    }

    #[tokio::test]
    async fn memory_same_target_is_noop() {
        let f = Fixture::new();
        let link = f.memory_link();
        fs::create_dir_all(link.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(f.path("src/memory"), &link).unwrap();
        f.manager.build(&f.with_memory()).await.unwrap();
        assert_eq!(fs::read_link(&link).unwrap(), f.path("src/memory"));
    }

    #[tokio::test]
    async fn memory_other_symlink_is_replaced_without_touching_old_target() {
        let f = Fixture::new();
        let link = f.memory_link();
        fs::create_dir_all(link.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(f.path("src/brain"), &link).unwrap();
        f.manager.build(&f.with_memory()).await.unwrap();
        assert_eq!(fs::read_link(&link).unwrap(), f.path("src/memory"));
        assert!(f.path("src/brain/notes.md").exists());
    }

    #[tokio::test]
    async fn memory_empty_dir_is_replaced() {
        let f = Fixture::new();
        let link = f.memory_link();
        fs::create_dir_all(&link).unwrap();
        f.manager.build(&f.with_memory()).await.unwrap();
        assert_eq!(fs::read_link(&link).unwrap(), f.path("src/memory"));
    }

    #[tokio::test]
    async fn memory_non_empty_dir_is_refused_and_kept() {
        let f = Fixture::new();
        let link = f.memory_link();
        fs::create_dir_all(&link).unwrap();
        fs::write(link.join("MEMORY.md"), "user").unwrap();
        let msg = precondition_msg(f.manager.build(&f.with_memory()).await.unwrap_err());
        assert!(msg.contains(&link.display().to_string()), "{msg}");
        assert_eq!(fs::read_to_string(link.join("MEMORY.md")).unwrap(), "user");
        assert!(!f.path("workspace/drive/w1").exists());
    }

    #[tokio::test]
    async fn teardown_removes_links_only_and_is_idempotent() {
        let f = Fixture::new();
        let req = LocalWorkspaceRequest {
            mounts: vec![mount(f.path("src/ctx"), "/a/ctx", true)],
            brain_dir: Some(f.path("src/brain")),
            skills: vec!["implement".into()],
            ..f.with_memory()
        };
        f.manager.build(&req).await.unwrap();
        let slot = f.path("slot");

        f.manager
            .teardown("w1", &slot, AgentType::Claude)
            .await
            .unwrap();
        assert!(!f.path("workspace/drive/w1").exists());
        assert!(fs::symlink_metadata(f.memory_link()).is_err());
        assert_eq!(
            fs::read_to_string(f.path("src/ctx/keep.txt")).unwrap(),
            "ctx"
        );
        assert_eq!(
            fs::read_to_string(f.path("src/brain/notes.md")).unwrap(),
            "brain"
        );
        assert_eq!(
            fs::read_to_string(f.path("src/memory/MEMORY.md")).unwrap(),
            "mem"
        );

        f.manager
            .teardown("w1", &slot, AgentType::Claude)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn teardown_rejects_escaping_worker_ids() {
        let f = Fixture::new();
        fs::create_dir_all(f.path("workspace/drive")).unwrap();
        for id in ["..", ".", "", "a/b", "/abs"] {
            let err = f
                .manager
                .teardown(id, &f.path("slot"), AgentType::Claude)
                .await
                .unwrap_err();
            precondition_msg(err);
        }
        assert!(f.path("workspace/drive").exists());
    }

    #[tokio::test]
    async fn teardown_leaves_real_memory_dir() {
        let f = Fixture::new();
        let link = f.memory_link();
        fs::create_dir_all(&link).unwrap();
        fs::write(link.join("MEMORY.md"), "user").unwrap();
        f.manager
            .teardown("w1", &f.path("slot"), AgentType::Claude)
            .await
            .unwrap();
        assert!(link.join("MEMORY.md").exists());
    }

    #[tokio::test]
    async fn installs_plugin_skill_hooks_and_git_hooks() {
        let f = Fixture::new();
        let req = LocalWorkspaceRequest {
            skills: vec!["implement".into()],
            ..f.request()
        };
        f.manager.build(&req).await.unwrap();
        let ur = f.path("workspace/drive/w1/.ur");
        assert!(ur.join("plugin/.claude-plugin/plugin.json").exists());
        assert_eq!(
            fs::read_to_string(ur.join("plugin/skills/implement/SKILL.md")).unwrap(),
            format!("hooks: {}/implement", ur.join("skill-hooks").display())
        );
        assert_eq!(
            fs::read_to_string(ur.join("skill-hooks/implement/pre.md")).unwrap(),
            "overlay"
        );
        assert!(f.path("slot/.git/hooks/pre-commit").exists());
        let settings: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(ur.join("settings.json")).unwrap()).unwrap();
        assert_eq!(settings, LocalSettingsManager.settings_json(false));
    }

    #[tokio::test]
    async fn instructions_golden() {
        let f = Fixture::new();
        let req = LocalWorkspaceRequest {
            mounts: vec![mount(f.path("src/ctx"), "/ctx", true)],
            brain_dir: Some(f.path("src/brain")),
            instruction_strategy: Some("code".into()),
            project_instruction_path: Some(f.path("slot/PROJECT.md")),
            ..f.request()
        };
        f.manager.build(&req).await.unwrap();
        let drive = f.path("workspace/drive/w1");
        let got = fs::read_to_string(drive.join(".ur/instructions.md")).unwrap();
        let d = drive.display();
        let s = f.path("src").display().to_string();
        let expected = format!(
            "# Code Worker\n\n# Tickets\n\n# Skill Precedence\n\n\
             Worker skills are installed as the `ur` plugin and are invoked as \
             `ur:<name>` (e.g. `/ur:implement`). When a skill exists as both \
             `ur:<name>` and `<name>`, ALWAYS use `ur:<name>`.\n\
             \n\nProject at {slot}\n\n\
             # Drive\n\nYour drive is `{d}`. It links these host paths:\n\n\
             - `{d}/ctx` -> `{s}/ctx` (read-only: advisory — do not write)\n\
             - `{d}/brain` -> `{s}/brain`\n\
             \n# Host Tools\n\n\
             You run directly on the host, not in a container. `git`, `gh` and other \
             tools run natively with the user's credentials; ur's host-exec policy is \
             NOT applied. Follow the same rules you would in a container: never merge, \
             close or approve pull requests, and never bypass git hooks.\n",
            slot = f.path("slot").display(),
        );
        assert_eq!(got, expected);
    }

    #[tokio::test]
    async fn instructions_without_strategy_hold_only_the_appendix() {
        let f = Fixture::new();
        f.manager.build(&f.request()).await.unwrap();
        let drive = f.path("workspace/drive/w1");
        let got = fs::read_to_string(drive.join(".ur/instructions.md")).unwrap();
        assert_eq!(got, LocalSettingsManager.instructions_appendix(&drive, &[]));
    }

    #[tokio::test]
    async fn argv_golden() {
        let f = Fixture::new();
        let req = LocalWorkspaceRequest {
            mounts: vec![mount(f.path("src/ctx"), "/ctx", true)],
            brain_dir: Some(f.path("src/brain")),
            model: Some("opus".into()),
            effort: "high".into(),
            ..f.request()
        };
        let built = f.manager.build(&req).await.unwrap();
        let d = f.path("workspace/drive/w1").display().to_string();
        let s = f.path("src").display().to_string();
        assert_eq!(
            built.argv,
            vec![
                "claude".to_owned(),
                "--settings".into(),
                format!("{d}/.ur/settings.json"),
                "--append-system-prompt-file".into(),
                format!("{d}/.ur/instructions.md"),
                "--plugin-dir".into(),
                format!("{d}/.ur/plugin"),
                "--add-dir".into(),
                d.clone(),
                "--add-dir".into(),
                format!("{s}/ctx"),
                "--add-dir".into(),
                format!("{s}/brain"),
                "--model".into(),
                "opus".into(),
                "--effort".into(),
                "high".into(),
            ]
        );
    }

    #[tokio::test]
    async fn argv_defaults_effort_and_omits_blank_model() {
        let f = Fixture::new();
        let req = LocalWorkspaceRequest {
            model: Some("  ".into()),
            ..f.request()
        };
        let built = f.manager.build(&req).await.unwrap();
        assert!(!built.argv.contains(&"--model".to_owned()));
        assert_eq!(built.argv[built.argv.len() - 2..], ["--effort", "medium"]);
    }

    #[tokio::test]
    async fn rebuild_replaces_stale_drive() {
        let f = Fixture::new();
        let stale = f.path("workspace/drive/w1/stale");
        fs::create_dir_all(&stale).unwrap();
        f.manager.build(&f.request()).await.unwrap();
        assert!(!stale.exists());
    }
}
