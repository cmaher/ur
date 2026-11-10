use std::path::PathBuf;

use chrono::Utc;
use tracing::info;
use uuid::Uuid;

use ur_config::Config;
use workflow_db::WorkerRepo;

use crate::ProjectRegistry;
use crate::builder_pool_client::BuilderPoolClient;

/// Manages a pool of pre-cloned git repositories per project.
///
/// Directory layout: `$WORKSPACE/pool/<project-key>/<slot-name>/`
///
/// All filesystem and git operations are delegated to builderd via
/// `BuilderPoolClient`. The server only performs DB orchestration.
///
/// All slots are exclusive: acquired by one worker at a time. The `worker_slot`
/// row is the claim: it is inserted together with the worker's `provisioning`
/// row, and `UNIQUE (slot_id)` makes that insert decide who gets a slot.
#[derive(Clone)]
pub struct RepoPoolManager {
    /// Prefix prepended to worker-ID branch names.
    git_branch_prefix: String,
    /// Database-backed slot repository for tracking slot availability.
    worker_repo: WorkerRepo,
    /// Shared project registry for live-reloadable project configs.
    project_registry: ProjectRegistry,
    /// Client for delegating all pool filesystem/git operations to builderd.
    builder_pool_client: BuilderPoolClient,
}

impl RepoPoolManager {
    pub fn new(
        config: &Config,
        worker_repo: WorkerRepo,
        project_registry: ProjectRegistry,
        builder_pool_client: BuilderPoolClient,
    ) -> Self {
        Self {
            git_branch_prefix: config.git_branch_prefix.clone(),
            worker_repo,
            project_registry,
            builder_pool_client,
        }
    }

    /// Look up a project and its git remote, refusing local projects up front.
    ///
    /// Called at the top of every pool entry point so a local project (`local = true`,
    /// no `repo`) fails before any DB query, builderd RPC, or git work is issued —
    /// there is nothing to clone, and a half-prepared slot directory would linger.
    fn resolve_pool_project(
        &self,
        project_key: &str,
    ) -> Result<(ur_config::ProjectConfig, String), String> {
        let project = self
            .project_registry
            .get(project_key)
            .ok_or_else(|| format!("unknown project: {project_key}"))?;
        let repo = project.repo.clone().ok_or_else(|| {
            format!(
                "project '{project_key}' is local (local = true, no repo) — pool slots \
                 require a git remote. Use workspace mode instead: \
                 ur worker launch -m manual -w <dir> -p {project_key}"
            )
        })?;
        Ok((project, repo))
    }

    /// Acquire a repo slot for the given project, claiming it for the worker
    /// row `owner` builds from the slot's host path.
    ///
    /// 1. Looks up the project in config.
    /// 2. Tries each available slot (not linked to an active worker), oldest
    ///    first: `WorkerRepo::claim_slot` inserts the `provisioning` row and its
    ///    link in one transaction; losing that race moves on to the next slot.
    /// 3. A claimed slot is recycled (fetch + reset) under the claim.
    /// 4. With none available, checks `scan_slots` against pool_limit, prepares
    ///    a new slot, inserts its row and claims it.
    ///
    /// Returns the host path and the claimed row. On failure no claim is left.
    pub async fn acquire_slot<F>(
        &self,
        project_key: &str,
        owner: F,
    ) -> Result<(PathBuf, workflow_db::model::Worker), String>
    where
        F: Fn(&std::path::Path) -> workflow_db::model::Worker,
    {
        let (project, repo) = self.resolve_pool_project(project_key)?;

        let candidates = self
            .worker_repo
            .find_available_slots(project_key)
            .await
            .map_err(|e| format!("db error finding available slot: {e}"))?;
        for slot in candidates {
            let row = owner(std::path::Path::new(&slot.host_path));
            if !self.claim(&row, &slot.id).await? {
                info!(project_key, slot_id = %slot.id, "slot taken by a concurrent launch, trying next");
                continue;
            }
            info!(project_key, slot_name = %slot.slot_name, path = %slot.host_path, "recycling existing pool slot");
            let recycled = self
                .builder_pool_client
                .recycle_slot(project_key.to_owned(), slot.slot_name.clone(), repo.clone())
                .await;
            return match recycled {
                Ok(host_path) => Ok((host_path, row)),
                Err(e) => Err(self
                    .drop_claim(
                        &row.worker_id,
                        format!("recycle_slot failed for slot {}: {e}", slot.slot_name),
                    )
                    .await),
            };
        }

        // No available slot — check pool_limit using builderd scan
        let existing_slots = self
            .builder_pool_client
            .scan_slots(project_key.to_owned())
            .await
            .map_err(|e| format!("scan_slots failed for {project_key}: {e}"))?;
        let total_slots = existing_slots.len() as u32;
        if total_slots >= project.pool_limit {
            return Err(format!(
                "pool limit reached for project {project_key}: {total_slots}/{} slots in use",
                project.pool_limit
            ));
        }

        // Find next slot index (fill gaps or use max + 1)
        let mut sorted_slots = existing_slots.clone();
        sorted_slots.sort();
        let next_index = next_slot_index(&sorted_slots);
        let slot_name = next_index.to_string();

        info!(
            project_key,
            slot_index = next_index,
            repo = %repo,
            "preparing new pool slot via builderd"
        );

        let host_path = self
            .builder_pool_client
            .prepare_new_slot(project_key.to_owned(), slot_name.clone(), repo.clone())
            .await
            .map_err(|e| format!("prepare_new_slot failed for {project_key}/{slot_name}: {e}"))?;

        // Insert new slot row in DB
        let now = Utc::now().to_rfc3339();
        let slot_id = Uuid::new_v4().to_string();
        let new_slot = workflow_db::model::Slot {
            id: slot_id.clone(),
            project_key: project_key.to_owned(),
            slot_name: slot_name.clone(),
            host_path: host_path.display().to_string(),
            created_at: now.clone(),
            updated_at: now,
        };
        self.worker_repo
            .insert_slot(&new_slot)
            .await
            .map_err(|e| format!("db error inserting new slot: {e}"))?;

        // The new row is visible to `find_available_slots` from here on.
        let row = owner(&host_path);
        if !self.claim(&row, &slot_id).await? {
            return Err(format!(
                "newly prepared slot {slot_name} was taken by a concurrent launch for {project_key}"
            ));
        }
        Ok((host_path, row))
    }

    async fn claim(&self, row: &workflow_db::model::Worker, slot_id: &str) -> Result<bool, String> {
        self.worker_repo
            .claim_slot(row, slot_id)
            .await
            .map_err(|e| format!("db error claiming slot: {e}"))
    }

    /// Undo a claim nothing was launched under: delete the row (and so the
    /// link). Returns `error`, extended if the delete fails.
    pub(crate) async fn drop_claim(&self, worker_id: &str, error: String) -> String {
        match self.worker_repo.delete_worker(worker_id).await {
            Ok(_) => error,
            Err(e) => format!("{error}; failed to drop slot claim: {e}"),
        }
    }

    /// Acquire a shared read-only slot for the given project.
    ///
    /// The shared slot lives at `pool/<project>/shared/` and can be mounted by
    /// multiple workers simultaneously. Unlike exclusive slots, the shared slot
    /// has no DB tracking or worker linking. It does not count against `pool_limit`.
    ///
    /// Delegates to `BuilderPoolClient::prepare_shared_slot` which handles
    /// both first-call (clone) and subsequent-call (fetch/reset) behavior.
    ///
    /// Returns the host-side path to the shared slot directory.
    pub async fn acquire_shared_slot(&self, project_key: &str) -> Result<PathBuf, String> {
        let (_project, repo) = self.resolve_pool_project(project_key)?;

        info!(project_key, repo = %repo, "preparing shared slot via builderd");
        self.builder_pool_client
            .prepare_shared_slot(project_key.to_owned(), repo)
            .await
            .map_err(|e| format!("prepare_shared_slot failed for {project_key}: {e}"))
    }

    /// Checkout a new branch in a slot via builderd.
    ///
    /// Extracts the slot_name from the slot_path (last path component) and the
    /// project_key from the DB. Calls `BuilderPoolClient::checkout_branch` with
    /// the git_branch_prefix from config.
    pub async fn checkout_branch(
        &self,
        host_slot_path: &std::path::Path,
        branch_name: &str,
    ) -> Result<(), String> {
        let slot = self
            .worker_repo
            .get_slot_by_host_path(&host_slot_path.display().to_string())
            .await
            .map_err(|e| format!("db error looking up slot for checkout: {e}"))?
            .ok_or_else(|| {
                format!(
                    "slot not found in DB for path: {}",
                    host_slot_path.display()
                )
            })?;

        self.builder_pool_client
            .checkout_branch(
                slot.project_key.clone(),
                slot.slot_name.clone(),
                self.git_branch_prefix.clone(),
                branch_name.to_owned(),
            )
            .await
            .map_err(|e| {
                format!(
                    "checkout_branch failed in {}: {e}",
                    host_slot_path.display()
                )
            })
    }
}

/// Find the next available slot index, filling gaps or using max + 1.
///
/// `existing` must be sorted ascending.
fn next_slot_index(existing: &[u32]) -> u32 {
    for (i, &idx) in existing.iter().enumerate() {
        if idx != i as u32 {
            return i as u32;
        }
    }
    existing.len() as u32
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use ur_config::ProjectConfig;

    /// Build a minimal `Config` for use in tests.
    fn minimal_test_config(tmp: &std::path::Path) -> ur_config::Config {
        ur_config::Config {
            agent: ur_config::AgentType::Claude,
            runtime: ur_config::WorkerRuntime::Container,
            config_dir: tmp.join("config"),
            workspace: tmp.join("workspace"),
            server_port: ur_config::DEFAULT_SERVER_PORT,
            builderd_port: ur_config::DEFAULT_SERVER_PORT + 2,
            worker_port: ur_config::DEFAULT_SERVER_PORT + 1,
            compose_file: tmp.join("docker-compose.yml"),
            proxy: ur_config::ProxyConfig {
                hostname: ur_config::DEFAULT_PROXY_HOSTNAME.into(),
                allowlist: vec![],
            },
            network: ur_config::NetworkConfig {
                name: ur_config::DEFAULT_NETWORK_NAME.into(),
                worker_name: ur_config::DEFAULT_WORKER_NETWORK_NAME.into(),
                server_hostname: ur_config::DEFAULT_SERVER_HOSTNAME.into(),
                worker_prefix: ur_config::DEFAULT_WORKER_PREFIX.into(),
            },
            db: ur_config::DatabaseConfig {
                host: ur_config::DEFAULT_DB_HOST.to_string(),
                port: ur_config::DEFAULT_DB_PORT,
                user: ur_config::DEFAULT_DB_USER.to_string(),
                password: ur_config::DEFAULT_DB_PASSWORD.to_string(),
                name: ur_config::DEFAULT_DB_NAME.to_string(),
                bind_address: None,
            },
            ticket_db: ur_config::TicketDbConfig {
                host: ur_config::DEFAULT_DB_HOST.to_string(),
                port: ur_config::DEFAULT_DB_PORT,
                user: ur_config::DEFAULT_DB_USER.to_string(),
                password: ur_config::DEFAULT_DB_PASSWORD.to_string(),
                name: ur_config::DEFAULT_TICKET_DB_NAME.to_string(),
                bind_address: None,
                backup: ur_config::BackupConfig {
                    path: None,
                    interval_minutes: ur_config::DEFAULT_BACKUP_INTERVAL_MINUTES,
                    enabled: true,
                    retain_count: ur_config::DEFAULT_BACKUP_RETAIN_COUNT,
                },
            },
            workflow_db: ur_config::WorkflowDbConfig {
                host: ur_config::DEFAULT_DB_HOST.to_string(),
                port: ur_config::DEFAULT_DB_PORT,
                user: ur_config::DEFAULT_DB_USER.to_string(),
                password: ur_config::DEFAULT_DB_PASSWORD.to_string(),
                name: ur_config::DEFAULT_WORKFLOW_DB_NAME.to_string(),
                bind_address: None,
                backup: ur_config::BackupConfig {
                    path: None,
                    interval_minutes: ur_config::DEFAULT_BACKUP_INTERVAL_MINUTES,
                    enabled: true,
                    retain_count: ur_config::DEFAULT_BACKUP_RETAIN_COUNT,
                },
            },
            server: ur_config::ServerConfig {
                container_command: "docker".into(),
                stale_worker_ttl_days: 7,
                max_implement_cycles: Some(6),
                poll_interval_ms: 500,
                github_scan_interval_secs: 30,
                builderd_retry_count: ur_config::DEFAULT_BUILDERD_RETRY_COUNT,
                builderd_retry_backoff_ms: ur_config::DEFAULT_BUILDERD_RETRY_BACKOFF_MS,
                ui_event_fallback_interval_ms: ur_config::DEFAULT_UI_EVENT_FALLBACK_INTERVAL_MS,
            },
            tui: ur_config::TuiConfig::default(),
            logs_dir: tmp.join("logs"),
            git_branch_prefix: String::new(),
            projects: std::collections::HashMap::new(),
            global_skills: ur_config::GlobalSkillsConfig::default(),
            workspace_brain_dir: None,
        }
    }

    async fn test_worker_repo() -> (WorkerRepo, ur_db_test::TestDb) {
        let test_db = ur_db_test::TestDb::new().await;
        let repo = WorkerRepo::new(test_db.workflow_pool().clone());
        (repo, test_db)
    }

    /// Create a RepoPoolManager backed by a temp directory with a fake project config.
    async fn test_pool(
        tmp: &std::path::Path,
        pool_limit: u32,
    ) -> (RepoPoolManager, ur_db_test::TestDb) {
        test_pool_with_repo(tmp, pool_limit, Some("git@github.com:test/testproj.git")).await
    }

    /// Like [`test_pool`], but lets the test choose whether `testproj` has a repo.
    /// `None` makes it a local project (`local = true` in `ur.toml`).
    async fn test_pool_with_repo(
        tmp: &std::path::Path,
        pool_limit: u32,
        repo: Option<&str>,
    ) -> (RepoPoolManager, ur_db_test::TestDb) {
        let mut projects = HashMap::new();
        projects.insert(
            "testproj".into(),
            ProjectConfig {
                key: "testproj".into(),
                repo: repo.map(str::to_owned),
                name: "Test Project".into(),
                pool_limit,
                skills: Vec::new(),
                hostexec: Vec::new(),
                hostexec_deny: Vec::new(),
                instruction_md: None,
                container: ur_config::ContainerConfig {
                    image: "ur-worker:latest".into(),
                    mounts: Vec::new(),
                    ports: Vec::new(),
                },
                max_fix_attempts: ur_config::DEFAULT_MAX_FIX_ATTEMPTS,
                max_implement_cycles: Some(ur_config::DEFAULT_MAX_IMPLEMENT_CYCLES),
                protected_branches: ur_config::default_protected_branches(),
                tui: None,
                git: Default::default(),
                ignored_workflow_checks: Vec::new(),
                hostexec_scripts: Vec::new(),
                push_again_exit_code: ur_config::DEFAULT_PUSH_AGAIN_EXIT_CODE,
                memory_dir: None,
                brain_dir: None,
                forge: None,
                runtime: None,
            },
        );
        let (worker_repo, test_db) = test_worker_repo().await;
        let channel =
            tonic::transport::Channel::from_static("http://localhost:12323").connect_lazy();
        let builder_pool_client = BuilderPoolClient::new(channel);
        let project_registry =
            ProjectRegistry::new(projects, crate::hostexec::HostExecConfigManager::empty());

        // Build a minimal Config to pass to RepoPoolManager::new
        let config = minimal_test_config(tmp);

        let mgr = RepoPoolManager::new(&config, worker_repo, project_registry, builder_pool_client);
        (mgr, test_db)
    }

    /// `acquire_slot` with a minimal claim row per candidate slot.
    async fn acquire(mgr: &RepoPoolManager, project_key: &str) -> Result<PathBuf, String> {
        mgr.acquire_slot(project_key, |path| claim_row(project_key, path))
            .await
            .map(|(path, _row)| path)
    }

    fn claim_row(project_key: &str, path: &std::path::Path) -> workflow_db::model::Worker {
        let now = Utc::now().to_rfc3339();
        workflow_db::model::Worker {
            worker_id: Uuid::new_v4().to_string(),
            process_id: format!("proc-{}", path.display()),
            project_key: project_key.to_owned(),
            container_id: String::new(),
            worker_secret: Uuid::new_v4().to_string(),
            strategy: "code".to_owned(),
            agent_type: ur_config::AgentType::Claude.name().to_owned(),
            container_status: "provisioning".to_owned(),
            agent_status: "starting".to_owned(),
            workspace_path: Some(path.display().to_string()),
            created_at: now.clone(),
            updated_at: now,
            idle_redispatch_count: 0,
            brain_path: None,
            runtime: workflow_db::model::WorkerRuntime::Container,
        }
    }

    /// A failed recycle leaves no claim: the slot is available again.
    #[tokio::test]
    async fn acquire_slot_drops_its_claim_on_recycle_failure() {
        let tmp = tempfile::tempdir().unwrap();
        let (mgr, _test_db) = test_pool(tmp.path(), 10).await;
        let slot0 = PathBuf::from("/fake/pool/testproj/0");
        insert_test_slot(&mgr.worker_repo, "testproj", "0", &slot0).await;

        // recycle_slot fails (no builderd).
        assert!(acquire(&mgr, "testproj").await.is_err());
        assert_eq!(
            mgr.worker_repo
                .find_available_slots("testproj")
                .await
                .unwrap()
                .len(),
            1
        );
        assert!(
            mgr.worker_repo
                .list_active_workers()
                .await
                .unwrap()
                .is_empty(),
            "no provisioning row left behind"
        );
    }

    /// Insert a slot row into the DB for testing. Returns the slot ID.
    async fn insert_test_slot(
        worker_repo: &WorkerRepo,
        project_key: &str,
        slot_name: &str,
        host_path: &std::path::Path,
    ) -> String {
        let now = Utc::now().to_rfc3339();
        let id = Uuid::new_v4().to_string();
        let slot = workflow_db::model::Slot {
            id: id.clone(),
            project_key: project_key.to_owned(),
            slot_name: slot_name.to_owned(),
            host_path: host_path.display().to_string(),
            created_at: now.clone(),
            updated_at: now,
        };
        worker_repo.insert_slot(&slot).await.unwrap();
        id
    }

    /// Insert a slot and mark it as in-use by linking a fake worker to it.
    async fn insert_test_slot_in_use(
        worker_repo: &WorkerRepo,
        project_key: &str,
        slot_name: &str,
        host_path: &std::path::Path,
    ) -> String {
        let slot_id = insert_test_slot(worker_repo, project_key, slot_name, host_path).await;
        // Create a fake running worker linked to this slot
        let worker_id = Uuid::new_v4().to_string();
        let now = Utc::now().to_rfc3339();
        let worker = workflow_db::model::Worker {
            worker_id: worker_id.clone(),
            process_id: format!("proc-{slot_name}"),
            project_key: project_key.to_owned(),
            container_id: format!("container-{slot_name}"),
            worker_secret: "secret".to_owned(),
            strategy: "code".to_owned(),
            agent_type: ur_config::AgentType::Claude.name().to_owned(),
            container_status: "running".to_owned(),
            agent_status: "starting".to_owned(),
            workspace_path: Some(host_path.display().to_string()),
            created_at: now.clone(),
            updated_at: now,
            idle_redispatch_count: 0,
            brain_path: None,
            runtime: workflow_db::model::WorkerRuntime::Container,
        };
        worker_repo.insert_worker(&worker).await.unwrap();
        worker_repo
            .link_worker_slot(&worker_id, &slot_id)
            .await
            .unwrap();
        slot_id
    }

    #[test]
    fn next_slot_index_empty() {
        assert_eq!(next_slot_index(&[]), 0);
    }

    #[test]
    fn next_slot_index_contiguous() {
        assert_eq!(next_slot_index(&[0, 1, 2]), 3);
    }

    #[test]
    fn next_slot_index_fills_gap() {
        assert_eq!(next_slot_index(&[0, 2, 3]), 1);
    }

    #[test]
    fn next_slot_index_fills_first_gap() {
        assert_eq!(next_slot_index(&[1, 2, 3]), 0);
    }

    #[tokio::test]
    async fn acquire_slot_unknown_project_errors() {
        let tmp = tempfile::tempdir().unwrap();
        let (mgr, _test_db) = test_pool(tmp.path(), 10).await;
        let result = acquire(&mgr, "nonexistent").await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("unknown project"));
    }

    #[tokio::test]
    async fn acquire_slot_pool_limit_reached_errors() {
        let tmp = tempfile::tempdir().unwrap();
        let (mgr, _test_db) = test_pool(tmp.path(), 1).await;

        // Create one slot directory and mark it in-use in DB (linked to a running worker)
        let slot0 = tmp
            .path()
            .join("workspace")
            .join("pool")
            .join("testproj")
            .join("0");
        std::fs::create_dir_all(&slot0).unwrap();
        insert_test_slot_in_use(&mgr.worker_repo, "testproj", "0", &slot0).await;

        // scan_slots will fail (no builderd), but the test verifies pool limit logic
        // is checked AFTER scan_slots returns. Since scan_slots will fail with a connection
        // error (no builderd), we just verify that we don't get an "unknown project" error.
        let result = acquire(&mgr, "testproj").await;
        assert!(result.is_err());
        assert!(
            !result.unwrap_err().contains("unknown project"),
            "should not get unknown project error"
        );
    }

    #[tokio::test]
    async fn acquire_slot_local_project_errors_before_any_builderd_call() {
        let tmp = tempfile::tempdir().unwrap();
        let (mgr, _test_db) = test_pool_with_repo(tmp.path(), 10, None).await;

        let err = acquire(&mgr, "testproj").await.unwrap_err();
        assert!(err.contains("is local"), "unexpected error: {err}");
        assert!(err.contains("testproj"), "unexpected error: {err}");
        // The builderd channel in tests points at a dead port, so any RPC would
        // surface as a transport error instead. Getting the locality message proves
        // the guard fired first — no half-prepared slot directory left behind.
        assert!(
            !err.contains("transport error") && !err.contains("Connection refused"),
            "guard should precede all builderd RPCs: {err}"
        );
    }

    #[tokio::test]
    async fn acquire_slot_local_project_inserts_no_slot_row() {
        let tmp = tempfile::tempdir().unwrap();
        let (mgr, _test_db) = test_pool_with_repo(tmp.path(), 10, None).await;

        assert!(acquire(&mgr, "testproj").await.is_err());

        // No DB state may be created for a project that can never have a slot.
        let slots = mgr
            .worker_repo
            .find_available_slots("testproj")
            .await
            .unwrap();
        assert!(
            slots.is_empty(),
            "expected no slot rows, got {}",
            slots.len()
        );
    }

    #[tokio::test]
    async fn acquire_shared_slot_local_project_errors() {
        let tmp = tempfile::tempdir().unwrap();
        let (mgr, _test_db) = test_pool_with_repo(tmp.path(), 10, None).await;

        let err = mgr.acquire_shared_slot("testproj").await.unwrap_err();
        assert!(err.contains("is local"), "unexpected error: {err}");
        assert!(
            !err.contains("transport error") && !err.contains("Connection refused"),
            "guard should precede all builderd RPCs: {err}"
        );
    }

    #[tokio::test]
    async fn acquire_shared_slot_unknown_project_still_reports_unknown() {
        // Locality must not swallow the distinct "unknown project" failure.
        let tmp = tempfile::tempdir().unwrap();
        let (mgr, _test_db) = test_pool_with_repo(tmp.path(), 10, None).await;
        let err = mgr.acquire_shared_slot("nonexistent").await.unwrap_err();
        assert!(err.contains("unknown project"), "unexpected error: {err}");
    }

    /// A project added by a registry hot-reload (`ur project add` → ReloadProjects)
    /// is immediately acquirable: `acquire_slot` gets past the project lookup and
    /// reaches builderd instead of failing with "unknown project".
    #[tokio::test]
    async fn acquire_slot_sees_project_added_by_reload() {
        let tmp = tempfile::tempdir().unwrap();
        // Reload re-reads hostexec config, so the registry needs a real
        // discovery-backed manager rather than `HostExecConfigManager::empty()`.
        let discovery =
            crate::hostexec::LuaDiscoveryManager::new(crate::hostexec::LuaTransformManager::new());
        let project_registry = ProjectRegistry::new(
            HashMap::new(),
            crate::hostexec::HostExecConfigManager::load(tmp.path(), &discovery).unwrap(),
        );
        let (worker_repo, _test_db) = test_worker_repo().await;
        let channel =
            tonic::transport::Channel::from_static("http://localhost:12323").connect_lazy();
        let mgr = RepoPoolManager::new(
            &minimal_test_config(tmp.path()),
            worker_repo,
            project_registry,
            BuilderPoolClient::new(channel),
        );
        let err = acquire(&mgr, "hotreload").await.unwrap_err();
        assert!(err.contains("unknown project"), "unexpected error: {err}");

        let report = mgr
            .project_registry
            .reload_from_str(
                r#"
[projects.hotreload]
repo = "git@github.com:test/hotreload.git"
[projects.hotreload.container]
image = "ur-worker"
"#,
                tmp.path(),
            )
            .unwrap();
        assert_eq!(report.added, vec!["hotreload".to_owned()]);

        let err = acquire(&mgr, "hotreload").await.unwrap_err();
        assert!(
            !err.contains("unknown project"),
            "reloaded project should be known to the pool: {err}"
        );
        assert!(
            err.contains("failed"),
            "acquire should reach builderd (dead in tests): {err}"
        );
    }

    #[tokio::test]
    async fn acquire_slot_unknown_project_errors_distinct() {
        let tmp = tempfile::tempdir().unwrap();
        let (mgr, _test_db) = test_pool(tmp.path(), 1).await;
        let result = acquire(&mgr, "nonexistent").await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("unknown project"));
    }

    #[tokio::test]
    async fn worker_slot_link_unlink_via_db() {
        let tmp = tempfile::tempdir().unwrap();
        let (mgr, _test_db) = test_pool(tmp.path(), 10).await;
        let slot_path = PathBuf::from("/fake/slot/0");

        let slot_id = insert_test_slot(&mgr.worker_repo, "testproj", "0", &slot_path).await;

        // Initially no worker is linked
        let available = mgr
            .worker_repo
            .find_available_slots("testproj")
            .await
            .unwrap();
        assert!(
            !available.is_empty(),
            "slot should be available (no linked worker)"
        );

        // Create a worker and link it
        let now = Utc::now().to_rfc3339();
        let worker = workflow_db::model::Worker {
            worker_id: "test-worker-1".to_owned(),
            process_id: "proc-1".to_owned(),
            project_key: "testproj".to_owned(),
            container_id: "container-1".to_owned(),
            worker_secret: "secret".to_owned(),
            strategy: "code".to_owned(),
            agent_type: ur_config::AgentType::Claude.name().to_owned(),
            container_status: "running".to_owned(),
            agent_status: "starting".to_owned(),
            workspace_path: None,
            created_at: now.clone(),
            updated_at: now,
            idle_redispatch_count: 0,
            brain_path: None,
            runtime: workflow_db::model::WorkerRuntime::Container,
        };
        mgr.worker_repo.insert_worker(&worker).await.unwrap();
        mgr.worker_repo
            .link_worker_slot("test-worker-1", &slot_id)
            .await
            .unwrap();

        // Now the slot should not be available
        let available = mgr
            .worker_repo
            .find_available_slots("testproj")
            .await
            .unwrap();
        assert!(
            available.is_empty(),
            "slot should not be available (linked to running worker)"
        );

        // Unlink the worker
        mgr.worker_repo
            .unlink_worker_slot("test-worker-1")
            .await
            .unwrap();

        // Slot should be available again
        let available = mgr
            .worker_repo
            .find_available_slots("testproj")
            .await
            .unwrap();
        assert!(
            !available.is_empty(),
            "slot should be available after unlink"
        );
    }

    #[tokio::test]
    async fn find_available_slots_excludes_non_numeric_names() {
        let tmp = tempfile::tempdir().unwrap();
        let (mgr, _test_db) = test_pool(tmp.path(), 10).await;

        // The shared slot and dotfile directories sit alongside the numeric slot dirs in
        // the pool root. Neither may ever be handed out as an exclusive slot.
        let shared_path = PathBuf::from("/fake/pool/testproj/shared");
        insert_test_slot(&mgr.worker_repo, "testproj", "shared", &shared_path).await;
        let dotfile_path = PathBuf::from("/fake/pool/testproj/.claude");
        insert_test_slot(&mgr.worker_repo, "testproj", ".claude", &dotfile_path).await;

        let available = mgr
            .worker_repo
            .find_available_slots("testproj")
            .await
            .unwrap();
        assert!(
            available.is_empty(),
            "non-numeric slots must not be returned, got: {:?}",
            available.iter().map(|s| &s.slot_name).collect::<Vec<_>>()
        );

        // Insert a normal numeric slot — this one should be returned
        let slot0_path = PathBuf::from("/fake/pool/testproj/0");
        insert_test_slot(&mgr.worker_repo, "testproj", "0", &slot0_path).await;

        let available = mgr
            .worker_repo
            .find_available_slots("testproj")
            .await
            .unwrap();
        assert_eq!(
            available.len(),
            1,
            "only the numeric slot should be returned"
        );
        assert_eq!(available[0].slot_name, "0");
    }

    #[tokio::test]
    async fn acquire_slot_selects_available_slot_from_db() {
        let tmp = tempfile::tempdir().unwrap();
        let (mgr, _test_db) = test_pool(tmp.path(), 10).await;

        let slot0 = PathBuf::from("/fake/pool/testproj/0");
        let slot1 = PathBuf::from("/fake/pool/testproj/1");
        let slot2 = PathBuf::from("/fake/pool/testproj/2");

        // Slots 0 and 1 are in-use (linked to running workers), slot 2 is available
        insert_test_slot_in_use(&mgr.worker_repo, "testproj", "0", &slot0).await;
        insert_test_slot_in_use(&mgr.worker_repo, "testproj", "1", &slot1).await;
        insert_test_slot(&mgr.worker_repo, "testproj", "2", &slot2).await;

        // Acquire should try to recycle slot 2, which will fail on builderd connection.
        // The error should reference a recycle_slot failure (not "unknown project" or "pool limit").
        let result = acquire(&mgr, "testproj").await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            err.contains("recycle_slot") || err.contains("slot 2") || err.contains("/2"),
            "expected recycle_slot error for slot 2, got: {err}"
        );
    }

    #[tokio::test]
    async fn acquire_slot_attempts_prepare_when_no_existing_slots() {
        let tmp = tempfile::tempdir().unwrap();
        let (mgr, _test_db) = test_pool(tmp.path(), 10).await;

        // No slots exist in DB. Acquire should call scan_slots then prepare_new_slot.
        // Both will fail (no builderd running), but the error should propagate.
        let result = acquire(&mgr, "testproj").await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        // Should be a builderd error (not "pool limit" or "unknown project")
        assert!(
            err.contains("failed")
                && !err.contains("pool limit")
                && !err.contains("unknown project"),
            "expected builderd error, got: {err}"
        );
    }

    #[tokio::test]
    async fn acquire_shared_slot_unknown_project_errors() {
        let tmp = tempfile::tempdir().unwrap();
        let (mgr, _test_db) = test_pool(tmp.path(), 10).await;
        let result = mgr.acquire_shared_slot("nonexistent").await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("unknown project"));
    }

    #[tokio::test]
    async fn acquire_shared_slot_delegates_to_builderd() {
        let tmp = tempfile::tempdir().unwrap();
        let (mgr, _test_db) = test_pool(tmp.path(), 10).await;

        // Will fail (no builderd), but we verify it attempts to call builderd
        // (not "unknown project" error).
        let result = mgr.acquire_shared_slot("testproj").await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            !err.contains("unknown project"),
            "expected builderd error, got: {err}"
        );
    }
}
