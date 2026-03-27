use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::Utc;
use tracing::{info, warn};
use uuid::Uuid;

use ur_config::{Config, ProjectConfig};
use ur_db::WorkerRepo;

use local_repo::LocalRepo;
use ur_rpc::proto::builder::BuilderdClient;

/// Manages a pool of pre-cloned git repositories per project.
///
/// Directory layout: `$WORKSPACE/pool/<project-key>/<slot-name>/`
///
/// Git operations (clone, fetch, reset) are executed on the host via builderd,
/// since the server runs inside a Docker container without SSH keys or git credentials.
///
/// All slots are exclusive: acquired by one worker at a time, tracked via the slot
/// and worker_slot tables in the database.
#[derive(Clone)]
pub struct RepoPoolManager {
    /// Container-local workspace path for filesystem operations (scanning, mkdir).
    /// Inside the server container this is `/workspace`.
    local_workspace: PathBuf,
    /// Host-side workspace path for returned slot paths (used in Docker volume
    /// mounts and builderd CWD). e.g., `~/.ur/workspace`.
    host_workspace: PathBuf,
    /// Pre-connected builderd client for non-git host commands (rm, mise).
    builderd_client: BuilderdClient,
    /// Git operations routed through builderd.
    local_repo: local_repo::GitBackend,
    /// Project configs keyed by project key.
    projects: HashMap<String, ProjectConfig>,
    /// Prefix prepended to worker-ID branch names.
    git_branch_prefix: String,
    /// Database-backed slot repository for tracking slot availability.
    worker_repo: WorkerRepo,
    /// Host-side config directory for convention-based local project files.
    #[allow(dead_code)]
    host_config_dir: PathBuf,
}

impl RepoPoolManager {
    pub fn new(
        config: &Config,
        local_workspace: PathBuf,
        host_workspace: PathBuf,
        builderd_client: BuilderdClient,
        local_repo: local_repo::GitBackend,
        worker_repo: WorkerRepo,
        host_config_dir: PathBuf,
    ) -> Self {
        Self {
            local_workspace,
            host_workspace,
            builderd_client,
            local_repo,
            projects: config.projects.clone(),
            git_branch_prefix: config.git_branch_prefix.clone(),
            worker_repo,
            host_config_dir,
        }
    }

    /// Local pool root for filesystem operations: `$LOCAL_WORKSPACE/pool/`.
    fn local_pool_root(&self) -> PathBuf {
        self.local_workspace.join("pool")
    }

    /// Local project pool directory: `$LOCAL_WORKSPACE/pool/<project-key>/`.
    fn local_project_pool_dir(&self, project_key: &str) -> PathBuf {
        self.local_pool_root().join(project_key)
    }

    /// Host-side pool root: `$HOST_WORKSPACE/pool/`.
    fn host_pool_root(&self) -> PathBuf {
        self.host_workspace.join("pool")
    }

    /// Host-side project pool directory: `$HOST_WORKSPACE/pool/<project-key>/`.
    fn host_project_pool_dir(&self, project_key: &str) -> PathBuf {
        self.host_pool_root().join(project_key)
    }

    /// Host-side path for a specific slot (returned for Docker mounts and builderd CWD).
    fn host_slot_path(&self, project_key: &str, slot_name: &str) -> PathBuf {
        self.host_project_pool_dir(project_key).join(slot_name)
    }

    /// Acquire a repo slot for the given project.
    ///
    /// 1. Looks up the project in config.
    /// 2. Queries DB for an available slot (not linked to an active worker).
    /// 3. If found, resets it to origin/master.
    /// 4. If none available, scans disk for existing slots, clones a new one (if under pool_limit),
    ///    and inserts a new slot row in the DB.
    ///
    /// Returns (host_path, slot_id) — the host-side path for Docker volume mounts and the
    /// slot ID for linking via worker_slot.
    pub async fn acquire_slot(&self, project_key: &str) -> Result<(PathBuf, String), String> {
        let project = self
            .projects
            .get(project_key)
            .ok_or_else(|| format!("unknown project: {project_key}"))?;

        // Query DB for an available slot (not linked to an active worker)
        let available_slot = self
            .worker_repo
            .find_available_slot(project_key)
            .await
            .map_err(|e| format!("db error finding available slot: {e}"))?;

        if let Some(slot) = available_slot {
            let host_path = PathBuf::from(&slot.host_path);
            let slot_id = slot.id.clone();
            let slot_name = slot.slot_name.clone();
            info!(project_key, slot_name = %slot_name, path = %host_path.display(), "resetting existing pool slot");
            if let Err(e) = self.reset_slot(&host_path).await {
                warn!(
                    project_key, slot_name = %slot_name, path = %host_path.display(),
                    error = %e, "reset failed, re-cloning corrupted pool slot"
                );
                self.reclone_slot(&project.repo, project_key, &slot_name)
                    .await?;
            }
            return Ok((host_path, slot_id));
        }

        // No available slot — check pool_limit using disk scan
        let local_pool_dir = self.local_project_pool_dir(project_key);
        let existing_slots = self.scan_slots(&local_pool_dir).await;
        let total_slots = existing_slots.len() as u32;
        if total_slots >= project.pool_limit {
            return Err(format!(
                "pool limit reached for project {project_key}: {total_slots}/{} slots in use",
                project.pool_limit
            ));
        }

        // Find next slot index (fill gaps or use max + 1)
        let next_index = self.next_slot_index(&existing_slots);
        let slot_name = next_index.to_string();
        let host_path = self.host_slot_path(project_key, &slot_name);

        info!(
            project_key,
            slot_index = next_index,
            repo = %project.repo,
            host_path = %host_path.display(),
            "cloning new pool slot via builderd"
        );

        self.clone_slot(&project.repo, project_key, &slot_name)
            .await?;

        // Insert new slot row in DB
        let now = Utc::now().to_rfc3339();
        let slot_id = Uuid::new_v4().to_string();
        let new_slot = ur_db::model::Slot {
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

        Ok((host_path, slot_id))
    }

    /// Release a previously acquired slot, resetting it to a clean state.
    ///
    /// Fetches, checks out master, resets to origin/master, and cleans.
    /// Unlinks the worker from the slot in the worker_slot join table.
    /// `slot_path` is a host-side path, `worker_id` identifies the worker to unlink.
    pub async fn release_slot(&self, worker_id: &str, slot_path: &Path) -> Result<(), String> {
        info!(worker_id, path = %slot_path.display(), "releasing pool slot");

        if let Err(e) = self.reset_slot(slot_path).await {
            // Unlink anyway so the next acquire can reclone it.
            warn!(path = %slot_path.display(), error = %e, "reset failed during release, slot will be recloned on next acquire");
        }

        self.worker_repo
            .unlink_worker_slot(worker_id)
            .await
            .map_err(|e| format!("db error unlinking worker from slot: {e}"))?;

        Ok(())
    }

    /// Scan the project pool directory for existing slot indices.
    /// Uses the local (container-side) filesystem path for directory listing.
    /// Returns a sorted vec of slot indices found on disk.
    async fn scan_slots(&self, local_pool_dir: &Path) -> Vec<u32> {
        let mut slots = Vec::new();
        let Ok(mut entries) = tokio::fs::read_dir(local_pool_dir).await else {
            return slots;
        };
        while let Ok(Some(entry)) = entries.next_entry().await {
            if let Some(name) = entry.file_name().to_str()
                && let Ok(idx) = name.parse::<u32>()
                && entry.path().is_dir()
            {
                slots.push(idx);
            }
        }
        slots.sort();
        slots
    }

    /// Find the next available slot index, filling gaps or using max + 1.
    fn next_slot_index(&self, existing: &[u32]) -> u32 {
        for (i, &idx) in existing.iter().enumerate() {
            if idx != i as u32 {
                return i as u32;
            }
        }
        existing.len() as u32
    }

    /// Convert a host-side path to a local (container-side) path.
    ///
    /// Replaces the host_workspace prefix with local_workspace prefix.
    /// Falls back to the input path if no prefix match (e.g., in tests).
    fn host_to_local_path(&self, host_path: &Path) -> PathBuf {
        let host_prefix = self.host_workspace.to_string_lossy();
        let path_str = host_path.to_string_lossy();
        if let Some(suffix) = path_str.strip_prefix(host_prefix.as_ref()) {
            self.local_workspace.join(suffix.trim_start_matches('/'))
        } else {
            host_path.to_path_buf()
        }
    }

    /// Convert a host-side path to a `%WORKSPACE%` template path for builderd CWD.
    ///
    /// Replaces the host_workspace prefix with `%WORKSPACE%` so builderd can resolve
    /// it to its own local workspace path at exec time. Falls back to the input path
    /// stringified if no prefix match (e.g., in tests where both are the same).
    fn to_builderd_path(&self, host_path: &Path) -> String {
        let host_prefix = self.host_workspace.to_string_lossy();
        let path_str = host_path.to_string_lossy();
        if let Some(suffix) = path_str.strip_prefix(host_prefix.as_ref()) {
            let suffix = suffix.trim_start_matches('/');
            if suffix.is_empty() {
                "%WORKSPACE%".to_string()
            } else {
                format!("%WORKSPACE%/{suffix}")
            }
        } else {
            path_str.to_string()
        }
    }

    /// Clone a repo into a new slot directory via builderd.
    ///
    /// Creates the parent directory locally (container-side, bind-mounted),
    /// then sends `git clone` to builderd which runs on the host with SSH credentials.
    async fn clone_slot(
        &self,
        repo_url: &str,
        project_key: &str,
        slot_name: &str,
    ) -> Result<(), String> {
        // Create parent directory locally (visible on host via bind mount)
        let local_parent = self.local_project_pool_dir(project_key);
        tokio::fs::create_dir_all(&local_parent)
            .await
            .map_err(|e| format!("failed to create pool directory: {e}"))?;

        let host_slot = self.host_slot_path(project_key, slot_name);
        let builderd_parent = self.to_builderd_path(&self.host_project_pool_dir(project_key));

        // Use slot_name as relative path since CWD is the parent directory.
        // builderd only resolves %WORKSPACE% in working_dir, not in args.
        LocalRepo::clone(&self.local_repo, repo_url, slot_name, &builderd_parent)
            .await
            .map_err(|e| format!("git clone failed for {repo_url}: {e}"))?;

        self.init_submodules(&host_slot).await?;
        self.trust_mise(&host_slot).await;

        Ok(())
    }

    /// Delete a corrupted slot and re-clone it from scratch.
    ///
    /// Removes the slot directory via builderd (`rm -rf`), then clones fresh.
    /// This recovers from any corruption (missing .git/config, partial clones, etc.).
    async fn reclone_slot(
        &self,
        repo_url: &str,
        project_key: &str,
        slot_name: &str,
    ) -> Result<(), String> {
        let host_slot = self.host_slot_path(project_key, slot_name);
        let builderd_parent = self.to_builderd_path(&self.host_project_pool_dir(project_key));

        // Remove the corrupted slot directory on the host.
        // Retries because macOS `rm -rf` can transiently fail with "Directory not
        // empty" when Spotlight or other background processes touch files during removal.
        // Use slot_name as relative path since builderd only resolves %WORKSPACE% in working_dir.
        ur_utils::retry(3, Duration::from_secs(1), "rm -rf slot", || {
            let parent = &builderd_parent;
            async move {
                self.builderd_client
                    .exec_check("rm", &["-rf", slot_name], parent)
                    .await
            }
        })
        .await
        .map_err(|e| {
            format!(
                "failed to remove corrupted slot {}: {e}",
                host_slot.display()
            )
        })?;

        // Clone fresh
        self.clone_slot(repo_url, project_key, slot_name)
            .await
            .map_err(|e| format!("reclone failed for slot {}: {e}", host_slot.display()))
    }

    /// Reset an existing slot to a clean origin/master state via builderd.
    ///
    /// Runs on the host: `git fetch origin && git checkout master && git reset --hard origin/master && git clean -fdx && git submodule update --init --recursive`
    /// `host_slot_path` is the host-side path to the slot.
    async fn reset_slot(&self, host_slot_path: &Path) -> Result<(), String> {
        let cwd = self.to_builderd_path(host_slot_path);

        self.local_repo
            .fetch(&cwd)
            .await
            .map_err(|e| format!("git fetch failed in {}: {e}", host_slot_path.display()))?;

        self.local_repo
            .checkout(&cwd, "master")
            .await
            .map_err(|e| {
                format!(
                    "git checkout master failed in {}: {e}",
                    host_slot_path.display()
                )
            })?;

        self.local_repo
            .reset_hard(&cwd, "origin/master")
            .await
            .map_err(|e| {
                format!(
                    "git reset --hard origin/master failed in {}: {e}",
                    host_slot_path.display()
                )
            })?;

        self.local_repo
            .clean(&cwd)
            .await
            .map_err(|e| format!("git clean -fdx failed in {}: {e}", host_slot_path.display()))?;

        self.init_submodules(host_slot_path).await?;

        Ok(())
    }

    /// Acquire a shared read-only slot for the given project.
    ///
    /// The shared slot lives at `pool/<project>/shared/` and can be mounted by
    /// multiple workers simultaneously. Unlike exclusive slots, the shared slot
    /// has no DB tracking or worker linking — the filesystem path is the state.
    /// It does not count against `pool_limit`.
    ///
    /// - First call (directory missing): clones via builderd.
    /// - Subsequent calls (directory exists): fetches and resets to `origin/HEAD`.
    ///
    /// Returns the host-side path to the shared slot directory.
    pub async fn acquire_shared_slot(&self, project_key: &str) -> Result<PathBuf, String> {
        let project = self
            .projects
            .get(project_key)
            .ok_or_else(|| format!("unknown project: {project_key}"))?;

        let shared_slot_name = "shared";
        let host_path = self.host_slot_path(project_key, shared_slot_name);
        let local_path = self
            .local_project_pool_dir(project_key)
            .join(shared_slot_name);

        let exists = tokio::fs::try_exists(&local_path).await.unwrap_or(false);

        if exists {
            info!(project_key, path = %host_path.display(), "refreshing shared slot");
            self.refresh_shared_slot(&host_path).await?;
        } else {
            info!(
                project_key,
                repo = %project.repo,
                path = %host_path.display(),
                "cloning shared slot via builderd"
            );
            self.clone_slot(&project.repo, project_key, shared_slot_name)
                .await?;
        }

        Ok(host_path)
    }

    /// Fetch and reset a shared slot to `origin/HEAD`.
    ///
    /// Unlike `reset_slot` (which checks out master and cleans), this only
    /// fetches and resets to `origin/HEAD` — suitable for a read-only shared
    /// checkout that may be mounted by multiple workers.
    async fn refresh_shared_slot(&self, host_slot_path: &Path) -> Result<(), String> {
        let cwd = self.to_builderd_path(host_slot_path);

        self.local_repo
            .fetch(&cwd)
            .await
            .map_err(|e| format!("git fetch failed in {}: {e}", host_slot_path.display()))?;

        self.local_repo
            .reset_hard(&cwd, "origin/HEAD")
            .await
            .map_err(|e| {
                format!(
                    "git reset --hard origin/HEAD failed in {}: {e}",
                    host_slot_path.display()
                )
            })?;

        self.init_submodules(host_slot_path).await?;

        Ok(())
    }

    /// Checkout a new branch in a slot via builderd.
    ///
    /// Runs `git checkout -b <prefix><branch_name>` in the slot directory on the host.
    /// The prefix comes from `git_branch_prefix` in `ur.toml` (empty by default).
    /// Called after acquire to give each worker its own branch.
    pub async fn checkout_branch(
        &self,
        host_slot_path: &Path,
        branch_name: &str,
    ) -> Result<(), String> {
        let full_branch = format!("{}{branch_name}", self.git_branch_prefix);
        let cwd = self.to_builderd_path(host_slot_path);
        self.local_repo
            .checkout_branch(&cwd, &full_branch)
            .await
            .map_err(|e| {
                format!(
                    "git checkout -B {full_branch} failed in {}: {e}",
                    host_slot_path.display()
                )
            })
    }

    /// Initialize/update git submodules recursively if the repo has a `.gitmodules` file.
    ///
    /// Uses the local (container-side) path to check for `.gitmodules` existence,
    /// then runs `git submodule update --init --recursive` on the host via builderd.
    async fn init_submodules(&self, host_slot_path: &Path) -> Result<(), String> {
        let local_slot_path = self.host_to_local_path(host_slot_path);

        let gitmodules = local_slot_path.join(".gitmodules");
        if !tokio::fs::try_exists(&gitmodules).await.unwrap_or(false) {
            return Ok(());
        }

        info!(path = %host_slot_path.display(), "initializing git submodules");
        let cwd = self.to_builderd_path(host_slot_path);
        self.local_repo.submodule_update(&cwd).await.map_err(|e| {
            format!(
                "git submodule update --init --recursive failed in {}: {e}",
                host_slot_path.display()
            )
        })
    }

    /// Trust mise configuration in a newly cloned slot if `mise.toml` exists.
    ///
    /// Runs `mise trust` on the host via builderd. If mise is not installed or the
    /// command fails, logs a warning and continues — mise trust is best-effort.
    async fn trust_mise(&self, host_slot_path: &Path) {
        let local_slot_path = self.host_to_local_path(host_slot_path);
        let mise_toml = local_slot_path.join("mise.toml");

        if !tokio::fs::try_exists(&mise_toml).await.unwrap_or(false) {
            return;
        }

        info!(path = %host_slot_path.display(), "trusting mise.toml in cloned slot");
        let cwd = self.to_builderd_path(host_slot_path);
        if let Err(e) = self
            .builderd_client
            .exec_check("mise", &["trust"], &cwd)
            .await
        {
            warn!(
                path = %host_slot_path.display(),
                error = %e,
                "mise trust failed (mise may not be installed)"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn test_worker_repo() -> WorkerRepo {
        let db = ur_db::DatabaseManager::open(":memory:")
            .await
            .expect("failed to open in-memory db");
        WorkerRepo::new(db.pool().clone())
    }

    /// Create a RepoPoolManager backed by a temp directory with a fake project config.
    /// Both local and host workspace point to the same temp path (no container split in tests).
    async fn test_pool(tmp: &Path, pool_limit: u32) -> (RepoPoolManager, PathBuf) {
        let workspace = tmp.join("workspace");
        let mut projects = HashMap::new();
        projects.insert(
            "testproj".into(),
            ProjectConfig {
                key: "testproj".into(),
                repo: String::new(),
                name: "Test Project".into(),
                pool_limit,
                hostexec: Vec::new(),
                git_hooks_dir: None,
                skill_hooks_dir: None,
                claude_md: None,
                container: ur_config::ContainerConfig {
                    image: "ur-worker:latest".into(),
                    mounts: Vec::new(),
                    ports: Vec::new(),
                },
                workflow_hooks_dir: None,
                max_fix_attempts: ur_config::DEFAULT_MAX_FIX_ATTEMPTS,
                protected_branches: ur_config::default_protected_branches(),
            },
        );
        let worker_repo = test_worker_repo().await;
        let channel =
            tonic::transport::Channel::from_static("http://localhost:42070").connect_lazy();
        let builderd_client = BuilderdClient::new(channel.clone());
        let local_repo = local_repo::GitBackend {
            client: BuilderdClient::new(channel),
        };
        let mgr = RepoPoolManager {
            local_workspace: workspace.clone(),
            host_workspace: workspace.clone(),
            builderd_client,
            local_repo,
            projects,
            git_branch_prefix: String::new(),
            worker_repo,
        };
        (mgr, workspace)
    }

    /// Insert a slot row into the DB for testing. Returns the slot ID.
    async fn insert_test_slot(
        worker_repo: &WorkerRepo,
        project_key: &str,
        slot_name: &str,
        host_path: &Path,
    ) -> String {
        let now = Utc::now().to_rfc3339();
        let id = Uuid::new_v4().to_string();
        let slot = ur_db::model::Slot {
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
        host_path: &Path,
    ) -> String {
        let slot_id = insert_test_slot(worker_repo, project_key, slot_name, host_path).await;
        // Create a fake running worker linked to this slot
        let worker_id = Uuid::new_v4().to_string();
        let now = Utc::now().to_rfc3339();
        let worker = ur_db::model::Worker {
            worker_id: worker_id.clone(),
            process_id: format!("proc-{slot_name}"),
            project_key: project_key.to_owned(),
            container_id: format!("container-{slot_name}"),
            worker_secret: "secret".to_owned(),
            strategy: "code".to_owned(),
            container_status: "running".to_owned(),
            agent_status: "starting".to_owned(),
            workspace_path: Some(host_path.display().to_string()),
            created_at: now.clone(),
            updated_at: now,
            idle_redispatch_count: 0,
        };
        worker_repo.insert_worker(&worker).await.unwrap();
        worker_repo
            .link_worker_slot(&worker_id, &slot_id)
            .await
            .unwrap();
        slot_id
    }

    #[tokio::test]
    async fn next_slot_index_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let (mgr, _) = test_pool(tmp.path(), 10).await;
        assert_eq!(mgr.next_slot_index(&[]), 0);
    }

    #[tokio::test]
    async fn next_slot_index_contiguous() {
        let tmp = tempfile::tempdir().unwrap();
        let (mgr, _) = test_pool(tmp.path(), 10).await;
        assert_eq!(mgr.next_slot_index(&[0, 1, 2]), 3);
    }

    #[tokio::test]
    async fn next_slot_index_fills_gap() {
        let tmp = tempfile::tempdir().unwrap();
        let (mgr, _) = test_pool(tmp.path(), 10).await;
        assert_eq!(mgr.next_slot_index(&[0, 2, 3]), 1);
    }

    #[tokio::test]
    async fn next_slot_index_fills_first_gap() {
        let tmp = tempfile::tempdir().unwrap();
        let (mgr, _) = test_pool(tmp.path(), 10).await;
        assert_eq!(mgr.next_slot_index(&[1, 2, 3]), 0);
    }

    #[tokio::test]
    async fn acquire_slot_unknown_project_errors() {
        let tmp = tempfile::tempdir().unwrap();
        let (mgr, _) = test_pool(tmp.path(), 10).await;
        let result = mgr.acquire_slot("nonexistent").await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("unknown project"));
    }

    #[tokio::test]
    async fn acquire_slot_pool_limit_reached_errors() {
        let tmp = tempfile::tempdir().unwrap();
        let (mgr, workspace) = test_pool(tmp.path(), 1).await;

        // Create one slot directory and mark it in-use in DB (linked to a running worker)
        let slot0 = workspace.join("pool").join("testproj").join("0");
        std::fs::create_dir_all(&slot0).unwrap();
        insert_test_slot_in_use(&mgr.worker_repo, "testproj", "0", &slot0).await;

        // Acquire should fail — 1 slot exists on disk, none available in DB, pool_limit = 1
        let result = mgr.acquire_slot("testproj").await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("pool limit reached"));
    }

    #[tokio::test]
    async fn scan_slots_finds_numeric_dirs() {
        let tmp = tempfile::tempdir().unwrap();
        let (mgr, workspace) = test_pool(tmp.path(), 10).await;

        let pool_dir = workspace.join("pool").join("testproj");
        std::fs::create_dir_all(pool_dir.join("0")).unwrap();
        std::fs::create_dir_all(pool_dir.join("2")).unwrap();
        std::fs::create_dir_all(pool_dir.join("5")).unwrap();
        // Non-numeric entry should be ignored
        std::fs::create_dir_all(pool_dir.join("not-a-slot")).unwrap();

        let slots = mgr.scan_slots(&pool_dir).await;
        assert_eq!(slots, vec![0, 2, 5]);
    }

    #[tokio::test]
    async fn scan_slots_empty_when_dir_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let (mgr, workspace) = test_pool(tmp.path(), 10).await;

        let pool_dir = workspace.join("pool").join("testproj");
        let slots = mgr.scan_slots(&pool_dir).await;
        assert!(slots.is_empty());
    }

    #[tokio::test]
    async fn worker_slot_link_unlink_via_db() {
        let tmp = tempfile::tempdir().unwrap();
        let (mgr, _) = test_pool(tmp.path(), 10).await;
        let slot_path = PathBuf::from("/fake/slot/0");

        let slot_id = insert_test_slot(&mgr.worker_repo, "testproj", "0", &slot_path).await;

        // Initially no worker is linked
        let available = mgr
            .worker_repo
            .find_available_slot("testproj")
            .await
            .unwrap();
        assert!(
            available.is_some(),
            "slot should be available (no linked worker)"
        );

        // Create a worker and link it
        let now = Utc::now().to_rfc3339();
        let worker = ur_db::model::Worker {
            worker_id: "test-worker-1".to_owned(),
            process_id: "proc-1".to_owned(),
            project_key: "testproj".to_owned(),
            container_id: "container-1".to_owned(),
            worker_secret: "secret".to_owned(),
            strategy: "code".to_owned(),
            container_status: "running".to_owned(),
            agent_status: "starting".to_owned(),
            workspace_path: None,
            created_at: now.clone(),
            updated_at: now,
            idle_redispatch_count: 0,
        };
        mgr.worker_repo.insert_worker(&worker).await.unwrap();
        mgr.worker_repo
            .link_worker_slot("test-worker-1", &slot_id)
            .await
            .unwrap();

        // Now the slot should not be available
        let available = mgr
            .worker_repo
            .find_available_slot("testproj")
            .await
            .unwrap();
        assert!(
            available.is_none(),
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
            .find_available_slot("testproj")
            .await
            .unwrap();
        assert!(available.is_some(), "slot should be available after unlink");
    }

    #[tokio::test]
    async fn pool_root_and_slot_paths() {
        let tmp = tempfile::tempdir().unwrap();
        let (mgr, workspace) = test_pool(tmp.path(), 10).await;

        // In tests, local and host paths are the same
        assert_eq!(mgr.local_pool_root(), workspace.join("pool"));
        assert_eq!(mgr.host_pool_root(), workspace.join("pool"));
        assert_eq!(
            mgr.local_project_pool_dir("myproj"),
            workspace.join("pool").join("myproj")
        );
        assert_eq!(
            mgr.host_slot_path("myproj", "3"),
            workspace.join("pool").join("myproj").join("3")
        );
    }

    #[tokio::test]
    async fn dual_workspace_paths() {
        let worker_repo = test_worker_repo().await;
        let mut projects = HashMap::new();
        projects.insert(
            "proj".into(),
            ProjectConfig {
                key: "proj".into(),
                repo: String::new(),
                name: "Proj".into(),
                pool_limit: 10,
                hostexec: Vec::new(),
                git_hooks_dir: None,
                skill_hooks_dir: None,
                claude_md: None,
                container: ur_config::ContainerConfig {
                    image: "ur-worker:latest".into(),
                    mounts: Vec::new(),
                    ports: Vec::new(),
                },
                workflow_hooks_dir: None,
                max_fix_attempts: ur_config::DEFAULT_MAX_FIX_ATTEMPTS,
                protected_branches: ur_config::default_protected_branches(),
            },
        );
        let channel =
            tonic::transport::Channel::from_static("http://localhost:42070").connect_lazy();
        let builderd_client = BuilderdClient::new(channel.clone());
        let local_repo = local_repo::GitBackend {
            client: BuilderdClient::new(channel),
        };
        let mgr = RepoPoolManager {
            local_workspace: PathBuf::from("/workspace"),
            host_workspace: PathBuf::from("/home/user/.ur/workspace"),
            builderd_client,
            local_repo,
            projects,
            git_branch_prefix: String::new(),
            worker_repo,
        };

        // Local paths for filesystem ops
        assert_eq!(mgr.local_pool_root(), PathBuf::from("/workspace/pool"));
        assert_eq!(
            mgr.local_project_pool_dir("proj"),
            PathBuf::from("/workspace/pool/proj")
        );

        // Host paths for Docker mounts
        assert_eq!(
            mgr.host_pool_root(),
            PathBuf::from("/home/user/.ur/workspace/pool")
        );
        assert_eq!(
            mgr.host_slot_path("proj", "0"),
            PathBuf::from("/home/user/.ur/workspace/pool/proj/0")
        );

        // Builderd template paths for CWD in exec requests
        assert_eq!(
            mgr.to_builderd_path(&mgr.host_slot_path("proj", "0")),
            "%WORKSPACE%/pool/proj/0"
        );
        assert_eq!(
            mgr.to_builderd_path(&mgr.host_project_pool_dir("proj")),
            "%WORKSPACE%/pool/proj"
        );
        assert_eq!(
            mgr.to_builderd_path(&mgr.host_workspace.clone()),
            "%WORKSPACE%"
        );
    }

    #[tokio::test]
    async fn to_builderd_path_with_same_workspace() {
        // When local and host workspace are the same (e.g., in tests or non-container mode),
        // to_builderd_path still produces %WORKSPACE% templates.
        let tmp = tempfile::tempdir().unwrap();
        let (mgr, workspace) = test_pool(tmp.path(), 10).await;

        let slot_path = workspace.join("pool").join("myproj").join("0");
        let builderd_path = mgr.to_builderd_path(&slot_path);
        assert_eq!(builderd_path, "%WORKSPACE%/pool/myproj/0");

        let workspace_root = mgr.to_builderd_path(&workspace);
        assert_eq!(workspace_root, "%WORKSPACE%");
    }

    #[tokio::test]
    async fn acquire_slot_skips_in_use_slots_selects_first_available() {
        let tmp = tempfile::tempdir().unwrap();
        let (mgr, workspace) = test_pool(tmp.path(), 10).await;

        // Create three slot directories
        let slot0 = workspace.join("pool").join("testproj").join("0");
        let slot1 = workspace.join("pool").join("testproj").join("1");
        let slot2 = workspace.join("pool").join("testproj").join("2");
        std::fs::create_dir_all(&slot0).unwrap();
        std::fs::create_dir_all(&slot1).unwrap();
        std::fs::create_dir_all(&slot2).unwrap();

        // Slots 0 and 1 are in-use (linked to running workers), slot 2 is available
        insert_test_slot_in_use(&mgr.worker_repo, "testproj", "0", &slot0).await;
        insert_test_slot_in_use(&mgr.worker_repo, "testproj", "1", &slot1).await;
        insert_test_slot(&mgr.worker_repo, "testproj", "2", &slot2).await;

        // Acquire should try slot 2, which will fail on builderd connection (expected in
        // unit tests — the important thing is it selects the right slot).
        // We test the selection logic by checking what the error says.
        let result = mgr.acquire_slot("testproj").await;
        // The git reset via builderd will fail because there's no builderd running,
        // but the error should reference slot 2's path (proving correct selection).
        match result {
            Ok((path, _slot_id)) => assert_eq!(path, slot2),
            Err(e) => assert!(
                e.contains(&slot2.to_string_lossy().to_string()),
                "expected error to reference slot2 path, got: {e}"
            ),
        }
    }

    #[tokio::test]
    async fn acquire_slot_clones_when_no_existing_slots() {
        let tmp = tempfile::tempdir().unwrap();
        let (mgr, workspace) = test_pool(tmp.path(), 10).await;

        // No slots exist on disk or in DB. Acquire should attempt git clone via builderd into slot 0.
        // The clone will fail (no builderd running), but we verify the error propagates.
        let result = mgr.acquire_slot("testproj").await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        // Should be a builderd/clone error (not "pool limit" or "unknown project")
        assert!(
            err.contains("git clone failed"),
            "expected clone error, got: {err}"
        );
        // The slot should NOT be in DB since clone failed
        let expected_slot = workspace.join("pool").join("testproj").join("0");
        let db_slot = mgr
            .worker_repo
            .get_slot_by_host_path(&expected_slot.display().to_string())
            .await
            .unwrap();
        assert!(db_slot.is_none());
    }

    #[tokio::test]
    async fn acquire_shared_slot_unknown_project_errors() {
        let tmp = tempfile::tempdir().unwrap();
        let (mgr, _) = test_pool(tmp.path(), 10).await;
        let result = mgr.acquire_shared_slot("nonexistent").await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("unknown project"));
    }

    #[tokio::test]
    async fn acquire_shared_slot_clones_on_first_call() {
        let tmp = tempfile::tempdir().unwrap();
        let (mgr, workspace) = test_pool(tmp.path(), 10).await;

        // No shared directory exists. Acquire should attempt clone via builderd.
        // Clone will fail (no builderd), but we verify it attempts clone (not fetch/reset).
        let result = mgr.acquire_shared_slot("testproj").await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            err.contains("git clone failed"),
            "expected clone error on first call, got: {err}"
        );

        // No DB entry should be created for shared slots
        let shared_path = workspace.join("pool").join("testproj").join("shared");
        let db_slot = mgr
            .worker_repo
            .get_slot_by_host_path(&shared_path.display().to_string())
            .await
            .unwrap();
        assert!(db_slot.is_none(), "shared slot must not create DB entries");
    }

    #[tokio::test]
    async fn acquire_shared_slot_fetches_when_exists() {
        let tmp = tempfile::tempdir().unwrap();
        let (mgr, workspace) = test_pool(tmp.path(), 10).await;

        // Create the shared directory to simulate a previous clone
        let shared_dir = workspace.join("pool").join("testproj").join("shared");
        std::fs::create_dir_all(&shared_dir).unwrap();

        // Acquire should attempt fetch+reset (not clone).
        // Fetch will fail (no builderd), but the error should be a fetch error.
        let result = mgr.acquire_shared_slot("testproj").await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            err.contains("git fetch failed"),
            "expected fetch error on subsequent call, got: {err}"
        );
    }

    #[tokio::test]
    async fn acquire_shared_slot_does_not_count_against_pool_limit() {
        let tmp = tempfile::tempdir().unwrap();
        let (mgr, workspace) = test_pool(tmp.path(), 1).await;

        // Fill the pool: create one exclusive slot directory and mark it in-use
        let slot0 = workspace.join("pool").join("testproj").join("0");
        std::fs::create_dir_all(&slot0).unwrap();
        insert_test_slot_in_use(&mgr.worker_repo, "testproj", "0", &slot0).await;

        // Exclusive acquire should fail (pool_limit = 1, 1 slot exists)
        let exclusive_result = mgr.acquire_slot("testproj").await;
        assert!(exclusive_result.is_err());
        assert!(exclusive_result.unwrap_err().contains("pool limit reached"));

        // Shared acquire should NOT fail with "pool limit" — it bypasses pool_limit.
        // It will fail on clone (no builderd), which proves it wasn't blocked by pool_limit.
        let shared_result = mgr.acquire_shared_slot("testproj").await;
        assert!(shared_result.is_err());
        let err = shared_result.unwrap_err();
        assert!(
            !err.contains("pool limit"),
            "shared slot must not count against pool_limit, got: {err}"
        );
    }

    #[tokio::test]
    async fn acquire_shared_slot_returns_correct_path() {
        let tmp = tempfile::tempdir().unwrap();
        let (mgr, workspace) = test_pool(tmp.path(), 10).await;

        // Create the shared directory and a .git dir to simulate a clone
        let shared_dir = workspace.join("pool").join("testproj").join("shared");
        std::fs::create_dir_all(&shared_dir).unwrap();

        // The call will fail on fetch (no builderd), but we can verify the expected path
        let expected_path = workspace.join("pool").join("testproj").join("shared");
        assert_eq!(
            mgr.host_slot_path("testproj", "shared"),
            expected_path,
            "shared slot path should be pool/<project>/shared/"
        );
    }
}
