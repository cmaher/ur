use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use tokio::sync::Mutex;
use tracing::{info, warn};

use ur_config::{Config, ProjectConfig};

use crate::BuilderdClient;

/// Per-slot mutexes for serializing concurrent shared slot acquires.
/// Keyed by (project_key, slot_name). Lock is held only during reset_slot,
/// not for the lifetime of the worker.
type SharedLockMap = Arc<RwLock<HashMap<(String, String), Arc<Mutex<()>>>>>;

/// Manages a pool of pre-cloned git repositories per project.
///
/// Directory layout: `$WORKSPACE/pool/<project-key>/<slot-name>/`
///
/// Git operations (clone, fetch, reset) are executed on the host via builderd,
/// since the server runs inside a Docker container without SSH keys or git credentials.
///
/// Supports two acquisition modes:
/// - **Exclusive** (numbered slots): Acquired by one agent at a time, tracked via `in_use` set.
/// - **Shared** (named slots): Multiple agents can use the same slot concurrently,
///   with per-slot mutexes serializing the initial reset.
///
/// In-memory tracking only — state is lost on restart.
#[derive(Clone)]
pub struct RepoPoolManager {
    /// Container-local workspace path for filesystem operations (scanning, mkdir).
    /// Inside the server container this is `/workspace`.
    local_workspace: PathBuf,
    /// Host-side workspace path for returned slot paths (used in Docker volume
    /// mounts and builderd CWD). e.g., `~/.ur/workspace`.
    host_workspace: PathBuf,
    /// Client for executing commands on the host via builderd.
    builderd_client: BuilderdClient,
    /// Project configs keyed by project key.
    projects: HashMap<String, ProjectConfig>,
    /// Set of slot paths (host-side) currently in use by running agents (exclusive slots only).
    in_use: Arc<RwLock<HashSet<PathBuf>>>,
    shared_locks: SharedLockMap,
}

impl RepoPoolManager {
    pub fn new(
        config: &Config,
        local_workspace: PathBuf,
        host_workspace: PathBuf,
        builderd_client: BuilderdClient,
    ) -> Self {
        Self {
            local_workspace,
            host_workspace,
            builderd_client,
            projects: config.projects.clone(),
            in_use: Arc::new(RwLock::new(HashSet::new())),
            shared_locks: Arc::new(RwLock::new(HashMap::new())),
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
    ///
    /// `slot_name` can be a numeric index (e.g., "0", "1") for exclusive slots
    /// or a named identifier (e.g., "design") for shared slots.
    fn host_slot_path(&self, project_key: &str, slot_name: &str) -> PathBuf {
        self.host_project_pool_dir(project_key).join(slot_name)
    }

    /// Acquire an exclusive repo slot for the given project.
    ///
    /// 1. Looks up the project in config.
    /// 2. Scans existing numbered slots for one not in use.
    /// 3. If found, resets it to origin/master and marks it in-use.
    /// 4. If none available, clones a new slot (if under pool_limit).
    ///
    /// Returns the host-side path to the acquired slot directory (for Docker volume mounts).
    pub async fn acquire_exclusive(&self, project_key: &str) -> Result<PathBuf, String> {
        let project = self
            .projects
            .get(project_key)
            .ok_or_else(|| format!("unknown project: {project_key}"))?;

        let local_pool_dir = self.local_project_pool_dir(project_key);

        // Scan existing slots (using local filesystem)
        let existing_slots = self.scan_slots(&local_pool_dir).await;

        // Find an available (not in-use) slot (tracked by host paths)
        let available_slot = {
            let in_use = self.in_use.read().expect("pool lock poisoned");
            existing_slots
                .iter()
                .find(|idx| !in_use.contains(&self.host_slot_path(project_key, &idx.to_string())))
                .copied()
        };

        if let Some(slot_index) = available_slot {
            let slot_name = slot_index.to_string();
            let host_path = self.host_slot_path(project_key, &slot_name);
            info!(project_key, slot_index, path = %host_path.display(), "resetting existing pool slot");
            if let Err(e) = self.reset_slot(&host_path).await {
                warn!(
                    project_key, slot_index, path = %host_path.display(),
                    error = %e, "reset failed, re-cloning corrupted pool slot"
                );
                self.reclone_slot(&project.repo, project_key, &slot_name)
                    .await?;
            }
            self.mark_in_use(&host_path);
            return Ok(host_path);
        }

        // No available slot — check pool_limit
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
        self.mark_in_use(&host_path);

        Ok(host_path)
    }

    /// Acquire a shared (named) repo slot for the given project.
    ///
    /// Shared slots are not tracked in the `in_use` set — multiple agents can hold
    /// the same slot concurrently. The slot is cloned on first use (if the directory
    /// doesn't exist) and reset to origin/master on each acquire.
    ///
    /// A per-slot mutex serializes the reset to avoid git lock conflicts when
    /// multiple agents acquire the same shared slot concurrently.
    pub async fn acquire_shared(
        &self,
        slot_name: &str,
        project_key: &str,
    ) -> Result<PathBuf, String> {
        let project = self
            .projects
            .get(project_key)
            .ok_or_else(|| format!("unknown project: {project_key}"))?;

        let host_path = self.host_slot_path(project_key, slot_name);
        let local_slot_dir = self.local_project_pool_dir(project_key).join(slot_name);

        // Get or create the per-slot mutex
        let slot_mutex = {
            let key = (project_key.to_string(), slot_name.to_string());
            let locks = self.shared_locks.read().expect("shared_locks poisoned");
            if let Some(mutex) = locks.get(&key) {
                mutex.clone()
            } else {
                drop(locks);
                let mut locks = self.shared_locks.write().expect("shared_locks poisoned");
                locks
                    .entry(key)
                    .or_insert_with(|| Arc::new(Mutex::new(())))
                    .clone()
            }
        };

        // Serialize reset/clone operations for this slot
        let _guard = slot_mutex.lock().await;

        let slot_exists = tokio::fs::try_exists(&local_slot_dir)
            .await
            .unwrap_or(false);

        if slot_exists {
            // Reset existing slot
            info!(project_key, slot_name, path = %host_path.display(), "resetting shared pool slot");
            if let Err(e) = self.reset_slot(&host_path).await {
                warn!(
                    project_key, slot_name, path = %host_path.display(),
                    error = %e, "reset failed on shared slot, re-cloning"
                );
                self.reclone_slot(&project.repo, project_key, slot_name)
                    .await?;
            }
        } else {
            // Clone on first use
            info!(
                project_key,
                slot_name,
                repo = %project.repo,
                host_path = %host_path.display(),
                "cloning new shared pool slot via builderd"
            );
            self.clone_slot(&project.repo, project_key, slot_name)
                .await?;
        }

        Ok(host_path)
    }

    /// Release a previously acquired exclusive slot, resetting it to a clean state.
    ///
    /// Fetches, checks out master, resets to origin/master, and cleans.
    /// Unmarks the slot from the in-use set.
    /// `slot_path` is a host-side path.
    pub async fn release_exclusive(&self, slot_path: &Path) -> Result<(), String> {
        info!(path = %slot_path.display(), "releasing exclusive pool slot");
        if let Err(e) = self.reset_slot(slot_path).await {
            // Mark available anyway so the next acquire can reclone it.
            warn!(path = %slot_path.display(), error = %e, "reset failed during release, slot will be recloned on next acquire");
            self.mark_available(slot_path);
            return Ok(());
        }
        self.mark_available(slot_path);
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
    ///
    /// `slot_name` can be a numeric index (e.g., "0") or a named identifier (e.g., "design").
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
        self.builderd_client
            .exec_and_check("git", &["clone", repo_url, slot_name], &builderd_parent)
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
    ///
    /// `slot_name` can be a numeric index (e.g., "0") or a named identifier (e.g., "design").
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
                    .exec_and_check("rm", &["-rf", slot_name], parent)
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
        let commands: &[&[&str]] = &[
            &["fetch", "origin"],
            &["checkout", "master"],
            &["reset", "--hard", "origin/master"],
            &["clean", "-fdx"],
        ];

        let cwd = self.to_builderd_path(host_slot_path);
        for args in commands {
            self.builderd_client
                .exec_and_check("git", args, &cwd)
                .await
                .map_err(|e| {
                    format!(
                        "git {} failed in {}: {e}",
                        args.join(" "),
                        host_slot_path.display()
                    )
                })?;
        }

        self.init_submodules(host_slot_path).await?;

        Ok(())
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
        self.builderd_client
            .exec_and_check(
                "git",
                &["submodule", "update", "--init", "--recursive"],
                &cwd,
            )
            .await
            .map_err(|e| {
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
            .exec_and_check("mise", &["trust"], &cwd)
            .await
        {
            warn!(
                path = %host_slot_path.display(),
                error = %e,
                "mise trust failed (mise may not be installed)"
            );
        }
    }

    /// Mark a slot path as in-use (host-side path).
    fn mark_in_use(&self, slot_path: &Path) {
        let mut in_use = self.in_use.write().expect("pool lock poisoned");
        in_use.insert(slot_path.to_path_buf());
    }

    /// Mark a slot path as available (host-side path).
    fn mark_available(&self, slot_path: &Path) {
        let mut in_use = self.in_use.write().expect("pool lock poisoned");
        in_use.remove(slot_path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Create a RepoPoolManager backed by a temp directory with a fake project config.
    /// Both local and host workspace point to the same temp path (no container split in tests).
    fn test_pool(tmp: &Path, pool_limit: u32) -> (RepoPoolManager, PathBuf) {
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
                mounts: Vec::new(),
            },
        );
        let mgr = RepoPoolManager {
            local_workspace: workspace.clone(),
            host_workspace: workspace.clone(),
            builderd_client: BuilderdClient::new("http://localhost:42070".into()),
            projects,
            in_use: Arc::new(RwLock::new(HashSet::new())),
            shared_locks: Arc::new(RwLock::new(HashMap::new())),
        };
        (mgr, workspace)
    }

    #[test]
    fn next_slot_index_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let (mgr, _) = test_pool(tmp.path(), 10);
        assert_eq!(mgr.next_slot_index(&[]), 0);
    }

    #[test]
    fn next_slot_index_contiguous() {
        let tmp = tempfile::tempdir().unwrap();
        let (mgr, _) = test_pool(tmp.path(), 10);
        assert_eq!(mgr.next_slot_index(&[0, 1, 2]), 3);
    }

    #[test]
    fn next_slot_index_fills_gap() {
        let tmp = tempfile::tempdir().unwrap();
        let (mgr, _) = test_pool(tmp.path(), 10);
        assert_eq!(mgr.next_slot_index(&[0, 2, 3]), 1);
    }

    #[test]
    fn next_slot_index_fills_first_gap() {
        let tmp = tempfile::tempdir().unwrap();
        let (mgr, _) = test_pool(tmp.path(), 10);
        assert_eq!(mgr.next_slot_index(&[1, 2, 3]), 0);
    }

    #[tokio::test]
    async fn acquire_exclusive_unknown_project_errors() {
        let tmp = tempfile::tempdir().unwrap();
        let (mgr, _) = test_pool(tmp.path(), 10);
        let result = mgr.acquire_exclusive("nonexistent").await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("unknown project"));
    }

    #[tokio::test]
    async fn acquire_exclusive_pool_limit_reached_errors() {
        let tmp = tempfile::tempdir().unwrap();
        let (mgr, workspace) = test_pool(tmp.path(), 1);

        // Create one slot directory and mark it in-use
        let slot0 = workspace.join("pool").join("testproj").join("0");
        std::fs::create_dir_all(&slot0).unwrap();
        mgr.mark_in_use(&slot0);

        // Acquire should fail — 1 slot exists, all in use, pool_limit = 1
        let result = mgr.acquire_exclusive("testproj").await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("pool limit reached"));
    }

    #[tokio::test]
    async fn scan_slots_finds_numeric_dirs() {
        let tmp = tempfile::tempdir().unwrap();
        let (mgr, workspace) = test_pool(tmp.path(), 10);

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
        let (mgr, workspace) = test_pool(tmp.path(), 10);

        let pool_dir = workspace.join("pool").join("testproj");
        let slots = mgr.scan_slots(&pool_dir).await;
        assert!(slots.is_empty());
    }

    #[test]
    fn mark_in_use_and_available() {
        let tmp = tempfile::tempdir().unwrap();
        let (mgr, _) = test_pool(tmp.path(), 10);
        let slot = PathBuf::from("/fake/slot/0");

        mgr.mark_in_use(&slot);
        assert!(mgr.in_use.read().unwrap().contains(&slot));

        mgr.mark_available(&slot);
        assert!(!mgr.in_use.read().unwrap().contains(&slot));
    }

    #[test]
    fn pool_root_and_slot_paths() {
        let tmp = tempfile::tempdir().unwrap();
        let (mgr, workspace) = test_pool(tmp.path(), 10);

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

    #[test]
    fn dual_workspace_paths() {
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
                mounts: Vec::new(),
            },
        );
        let mgr = RepoPoolManager {
            local_workspace: PathBuf::from("/workspace"),
            host_workspace: PathBuf::from("/home/user/.ur/workspace"),
            builderd_client: BuilderdClient::new("http://localhost:42070".into()),
            projects,
            in_use: Arc::new(RwLock::new(HashSet::new())),
            shared_locks: Arc::new(RwLock::new(HashMap::new())),
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

    #[test]
    fn to_builderd_path_with_same_workspace() {
        // When local and host workspace are the same (e.g., in tests or non-container mode),
        // to_builderd_path still produces %WORKSPACE% templates.
        let tmp = tempfile::tempdir().unwrap();
        let (mgr, workspace) = test_pool(tmp.path(), 10);

        let slot_path = workspace.join("pool").join("myproj").join("0");
        let builderd_path = mgr.to_builderd_path(&slot_path);
        assert_eq!(builderd_path, "%WORKSPACE%/pool/myproj/0");

        let workspace_root = mgr.to_builderd_path(&workspace);
        assert_eq!(workspace_root, "%WORKSPACE%");
    }

    #[tokio::test]
    async fn acquire_exclusive_skips_in_use_slots_selects_first_available() {
        let tmp = tempfile::tempdir().unwrap();
        let (mgr, workspace) = test_pool(tmp.path(), 10);

        // Create three slot directories
        let slot0 = workspace.join("pool").join("testproj").join("0");
        let slot1 = workspace.join("pool").join("testproj").join("1");
        let slot2 = workspace.join("pool").join("testproj").join("2");
        std::fs::create_dir_all(&slot0).unwrap();
        std::fs::create_dir_all(&slot1).unwrap();
        std::fs::create_dir_all(&slot2).unwrap();

        // Mark slots 0 and 1 as in-use (host paths = local paths in tests)
        mgr.mark_in_use(&slot0);
        mgr.mark_in_use(&slot1);

        // Acquire should try slot 2, which will fail on builderd connection (expected in
        // unit tests — the important thing is it selects the right slot).
        // We test the selection logic by checking what the error says.
        let result = mgr.acquire_exclusive("testproj").await;
        // The git reset via builderd will fail because there's no builderd running,
        // but the error should reference slot 2's path (proving correct selection).
        match result {
            Ok(path) => assert_eq!(path, slot2),
            Err(e) => assert!(
                e.contains(&slot2.to_string_lossy().to_string()),
                "expected error to reference slot2 path, got: {e}"
            ),
        }
    }

    #[tokio::test]
    async fn acquire_exclusive_clones_when_no_existing_slots() {
        let tmp = tempfile::tempdir().unwrap();
        let (mgr, workspace) = test_pool(tmp.path(), 10);

        // No slots exist on disk. Acquire should attempt git clone via builderd into slot 0.
        // The clone will fail (no builderd running), but we verify the error propagates.
        let result = mgr.acquire_exclusive("testproj").await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        // Should be a builderd/clone error (not "pool limit" or "unknown project")
        assert!(
            err.contains("git clone failed"),
            "expected clone error, got: {err}"
        );
        // The slot should NOT be marked in-use since clone failed
        let expected_slot = workspace.join("pool").join("testproj").join("0");
        assert!(!mgr.in_use.read().unwrap().contains(&expected_slot));
    }

    #[tokio::test]
    async fn acquire_shared_unknown_project_errors() {
        let tmp = tempfile::tempdir().unwrap();
        let (mgr, _) = test_pool(tmp.path(), 10);
        let result = mgr.acquire_shared("design", "nonexistent").await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("unknown project"));
    }

    #[tokio::test]
    async fn acquire_shared_clones_on_first_use() {
        let tmp = tempfile::tempdir().unwrap();
        let (mgr, workspace) = test_pool(tmp.path(), 10);

        // No slot exists on disk. acquire_shared should attempt git clone via builderd.
        // The clone will fail (no builderd running), but we verify the right path is used.
        let result = mgr.acquire_shared("design", "testproj").await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            err.contains("git clone failed"),
            "expected clone error, got: {err}"
        );
        // The slot should NOT be in the in_use set (shared slots are not tracked)
        let expected_slot = workspace.join("pool").join("testproj").join("design");
        assert!(!mgr.in_use.read().unwrap().contains(&expected_slot));
    }

    #[tokio::test]
    async fn acquire_shared_resets_existing_slot() {
        let tmp = tempfile::tempdir().unwrap();
        let (mgr, workspace) = test_pool(tmp.path(), 10);

        // Create the shared slot directory so it appears to already exist
        let design_slot = workspace.join("pool").join("testproj").join("design");
        std::fs::create_dir_all(&design_slot).unwrap();

        // acquire_shared should attempt to reset (not clone).
        // The reset will fail (no builderd running), then it will attempt reclone.
        let result = mgr.acquire_shared("design", "testproj").await;
        assert!(result.is_err());
        // The error comes from the reclone fallback path
        let err = result.unwrap_err();
        assert!(
            err.contains("failed to remove corrupted slot") || err.contains("reclone failed"),
            "expected reclone error, got: {err}"
        );
    }

    #[tokio::test]
    async fn acquire_shared_does_not_affect_in_use_tracking() {
        let tmp = tempfile::tempdir().unwrap();
        let (mgr, workspace) = test_pool(tmp.path(), 10);

        // Create the shared slot directory
        let design_slot = workspace.join("pool").join("testproj").join("design");
        std::fs::create_dir_all(&design_slot).unwrap();

        // Even after acquire_shared (which will fail on builderd), the in_use set remains empty
        let _ = mgr.acquire_shared("design", "testproj").await;
        assert!(mgr.in_use.read().unwrap().is_empty());
    }

    #[tokio::test]
    async fn acquire_shared_returns_same_path_on_subsequent_calls() {
        let tmp = tempfile::tempdir().unwrap();
        let (mgr, workspace) = test_pool(tmp.path(), 10);

        // Both calls target the same named slot — they should produce the same host path.
        // First call: slot doesn't exist, tries to clone (fails on builderd).
        let result1 = mgr.acquire_shared("design", "testproj").await;
        assert!(result1.is_err());

        // Create the slot directory to simulate a successful first clone.
        let design_slot = workspace.join("pool").join("testproj").join("design");
        std::fs::create_dir_all(&design_slot).unwrap();

        // Second call: slot exists, tries to reset (fails on builderd), then reclone.
        let result2 = mgr.acquire_shared("design", "testproj").await;
        assert!(result2.is_err());

        // Both attempts target the same host path.
        let expected = mgr.host_slot_path("testproj", "design");
        assert_eq!(expected, design_slot);
    }

    #[tokio::test]
    async fn shared_slots_do_not_consume_exclusive_capacity() {
        let tmp = tempfile::tempdir().unwrap();
        let (mgr, workspace) = test_pool(tmp.path(), 1);

        // Create a shared "design" slot directory
        let design_slot = workspace.join("pool").join("testproj").join("design");
        std::fs::create_dir_all(&design_slot).unwrap();

        // Attempt shared acquire (will fail on builderd, but that's fine)
        let _ = mgr.acquire_shared("design", "testproj").await;

        // scan_slots only counts numeric directories — "design" is ignored
        let pool_dir = workspace.join("pool").join("testproj");
        let slots = mgr.scan_slots(&pool_dir).await;
        assert!(
            slots.is_empty(),
            "shared slot should not appear in numeric scan"
        );

        // Exclusive acquire should still be allowed (pool_limit=1, no numeric slots exist)
        // It will fail on builderd clone, but the error should be a clone error, not pool limit
        let result = mgr.acquire_exclusive("testproj").await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            err.contains("git clone failed"),
            "expected clone error (not pool limit), got: {err}"
        );
    }

    #[test]
    fn host_slot_path_works_with_named_slots() {
        let tmp = tempfile::tempdir().unwrap();
        let (mgr, workspace) = test_pool(tmp.path(), 10);

        // Named slots use the name directly as the directory
        assert_eq!(
            mgr.host_slot_path("testproj", "design"),
            workspace.join("pool").join("testproj").join("design")
        );
        // Numeric slots still work
        assert_eq!(
            mgr.host_slot_path("testproj", "0"),
            workspace.join("pool").join("testproj").join("0")
        );
    }
}
