use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use tracing::{info, warn};

use ur_config::{Config, ProjectConfig};

use crate::HostdClient;

/// Manages a pool of pre-cloned git repositories per project.
///
/// Directory layout: `$WORKSPACE/pool/<project-key>/<slot-index>/`
///
/// Git operations (clone, fetch, reset) are executed on the host via ur-hostd,
/// since the server runs inside a Docker container without SSH keys or git credentials.
///
/// Slots are acquired for agent processes and released when processes stop.
/// In-memory tracking only — state is lost on restart.
#[derive(Clone)]
pub struct RepoPoolManager {
    /// Container-local workspace path for filesystem operations (scanning, mkdir).
    /// Inside the server container this is `/workspace`.
    local_workspace: PathBuf,
    /// Host-side workspace path for returned slot paths (used in Docker volume
    /// mounts and ur-hostd CWD). e.g., `~/.ur/workspace`.
    host_workspace: PathBuf,
    /// Client for executing commands on the host via ur-hostd.
    hostd_client: HostdClient,
    /// Project configs keyed by project key.
    projects: HashMap<String, ProjectConfig>,
    /// Set of slot paths (host-side) currently in use by running agents.
    in_use: Arc<RwLock<HashSet<PathBuf>>>,
}

impl RepoPoolManager {
    pub fn new(
        config: &Config,
        local_workspace: PathBuf,
        host_workspace: PathBuf,
        hostd_client: HostdClient,
    ) -> Self {
        Self {
            local_workspace,
            host_workspace,
            hostd_client,
            projects: config.projects.clone(),
            in_use: Arc::new(RwLock::new(HashSet::new())),
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

    /// Host-side path for a specific slot (returned for Docker mounts and hostd CWD).
    fn host_slot_path(&self, project_key: &str, slot_index: u32) -> PathBuf {
        self.host_project_pool_dir(project_key)
            .join(slot_index.to_string())
    }

    /// Acquire a repo slot for the given project.
    ///
    /// 1. Looks up the project in config.
    /// 2. Scans existing slots for one not in use.
    /// 3. If found, resets it to origin/master and marks it in-use.
    /// 4. If none available, clones a new slot (if under pool_limit).
    ///
    /// Returns the host-side path to the acquired slot directory (for Docker volume mounts).
    pub async fn acquire(&self, project_key: &str) -> Result<PathBuf, String> {
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
                .find(|idx| !in_use.contains(&self.host_slot_path(project_key, **idx)))
                .copied()
        };

        if let Some(slot_index) = available_slot {
            let host_path = self.host_slot_path(project_key, slot_index);
            info!(project_key, slot_index, path = %host_path.display(), "resetting existing pool slot");
            if let Err(e) = self.reset_slot(&host_path).await {
                warn!(
                    project_key, slot_index, path = %host_path.display(),
                    error = %e, "reset failed, re-cloning corrupted pool slot"
                );
                self.reclone_slot(&project.repo, project_key, slot_index)
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
        let host_path = self.host_slot_path(project_key, next_index);

        info!(
            project_key,
            slot_index = next_index,
            repo = %project.repo,
            host_path = %host_path.display(),
            "cloning new pool slot via hostd"
        );

        self.clone_slot(&project.repo, project_key, next_index)
            .await?;
        self.mark_in_use(&host_path);

        Ok(host_path)
    }

    /// Release a previously acquired slot, resetting it to a clean state.
    ///
    /// Fetches, checks out master, resets to origin/master, and cleans.
    /// `slot_path` is a host-side path.
    pub async fn release(&self, slot_path: &Path) -> Result<(), String> {
        info!(path = %slot_path.display(), "releasing pool slot");
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

    /// Clone a repo into a new slot directory via ur-hostd.
    ///
    /// Creates the parent directory locally (container-side, bind-mounted),
    /// then sends `git clone` to hostd which runs on the host with SSH credentials.
    async fn clone_slot(
        &self,
        repo_url: &str,
        project_key: &str,
        slot_index: u32,
    ) -> Result<(), String> {
        // Create parent directory locally (visible on host via bind mount)
        let local_parent = self.local_project_pool_dir(project_key);
        tokio::fs::create_dir_all(&local_parent)
            .await
            .map_err(|e| format!("failed to create pool directory: {e}"))?;

        let host_slot = self.host_slot_path(project_key, slot_index);
        let host_parent = self.host_project_pool_dir(project_key);

        self.hostd_client
            .exec_and_check(
                "git",
                &["clone", repo_url, &host_slot.to_string_lossy()],
                &host_parent.to_string_lossy(),
            )
            .await
            .map_err(|e| format!("git clone failed for {repo_url}: {e}"))?;

        self.init_submodules(&host_slot).await?;

        Ok(())
    }

    /// Delete a corrupted slot and re-clone it from scratch.
    ///
    /// Removes the slot directory via hostd (`rm -rf`), then clones fresh.
    /// This recovers from any corruption (missing .git/config, partial clones, etc.).
    async fn reclone_slot(
        &self,
        repo_url: &str,
        project_key: &str,
        slot_index: u32,
    ) -> Result<(), String> {
        let host_slot = self.host_slot_path(project_key, slot_index);
        let host_parent = self.host_project_pool_dir(project_key);

        // Remove the corrupted slot directory on the host
        self.hostd_client
            .exec_and_check(
                "rm",
                &["-rf", &host_slot.to_string_lossy()],
                &host_parent.to_string_lossy(),
            )
            .await
            .map_err(|e| {
                format!(
                    "failed to remove corrupted slot {}: {e}",
                    host_slot.display()
                )
            })?;

        // Clone fresh
        self.clone_slot(repo_url, project_key, slot_index)
            .await
            .map_err(|e| format!("reclone failed for slot {}: {e}", host_slot.display()))
    }

    /// Reset an existing slot to a clean origin/master state via ur-hostd.
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

        let cwd = host_slot_path.to_string_lossy();
        for args in commands {
            self.hostd_client
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
    /// then runs `git submodule update --init --recursive` on the host via hostd.
    async fn init_submodules(&self, host_slot_path: &Path) -> Result<(), String> {
        // Convert host path to local path to check for .gitmodules on the container filesystem.
        // host_workspace prefix is replaced with local_workspace prefix.
        let host_prefix = self.host_workspace.to_string_lossy();
        let slot_str = host_slot_path.to_string_lossy();
        let local_slot_path = if let Some(suffix) = slot_str.strip_prefix(host_prefix.as_ref()) {
            self.local_workspace.join(suffix.trim_start_matches('/'))
        } else {
            // Fallback: assume host and local paths are the same (e.g., in tests)
            host_slot_path.to_path_buf()
        };

        let gitmodules = local_slot_path.join(".gitmodules");
        if !tokio::fs::try_exists(&gitmodules).await.unwrap_or(false) {
            return Ok(());
        }

        info!(path = %host_slot_path.display(), "initializing git submodules");
        let cwd = host_slot_path.to_string_lossy();
        self.hostd_client
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
            hostd_client: HostdClient::new("http://localhost:42070".into()),
            projects,
            in_use: Arc::new(RwLock::new(HashSet::new())),
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
    async fn acquire_unknown_project_errors() {
        let tmp = tempfile::tempdir().unwrap();
        let (mgr, _) = test_pool(tmp.path(), 10);
        let result = mgr.acquire("nonexistent").await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("unknown project"));
    }

    #[tokio::test]
    async fn acquire_pool_limit_reached_errors() {
        let tmp = tempfile::tempdir().unwrap();
        let (mgr, workspace) = test_pool(tmp.path(), 1);

        // Create one slot directory and mark it in-use
        let slot0 = workspace.join("pool").join("testproj").join("0");
        std::fs::create_dir_all(&slot0).unwrap();
        mgr.mark_in_use(&slot0);

        // Acquire should fail — 1 slot exists, all in use, pool_limit = 1
        let result = mgr.acquire("testproj").await;
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
            mgr.host_slot_path("myproj", 3),
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
            hostd_client: HostdClient::new("http://localhost:42070".into()),
            projects,
            in_use: Arc::new(RwLock::new(HashSet::new())),
        };

        // Local paths for filesystem ops
        assert_eq!(mgr.local_pool_root(), PathBuf::from("/workspace/pool"));
        assert_eq!(
            mgr.local_project_pool_dir("proj"),
            PathBuf::from("/workspace/pool/proj")
        );

        // Host paths for Docker mounts and hostd
        assert_eq!(
            mgr.host_pool_root(),
            PathBuf::from("/home/user/.ur/workspace/pool")
        );
        assert_eq!(
            mgr.host_slot_path("proj", 0),
            PathBuf::from("/home/user/.ur/workspace/pool/proj/0")
        );
    }

    #[tokio::test]
    async fn acquire_skips_in_use_slots_selects_first_available() {
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

        // Acquire should try slot 2, which will fail on hostd connection (expected in
        // unit tests — the important thing is it selects the right slot).
        // We test the selection logic by checking what the error says.
        let result = mgr.acquire("testproj").await;
        // The git reset via hostd will fail because there's no hostd running,
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
    async fn acquire_clones_when_no_existing_slots() {
        let tmp = tempfile::tempdir().unwrap();
        let (mgr, workspace) = test_pool(tmp.path(), 10);

        // No slots exist on disk. Acquire should attempt git clone via hostd into slot 0.
        // The clone will fail (no hostd running), but we verify the error propagates.
        let result = mgr.acquire("testproj").await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        // Should be a hostd/clone error (not "pool limit" or "unknown project")
        assert!(
            err.contains("git clone failed"),
            "expected clone error, got: {err}"
        );
        // The slot should NOT be marked in-use since clone failed
        let expected_slot = workspace.join("pool").join("testproj").join("0");
        assert!(!mgr.in_use.read().unwrap().contains(&expected_slot));
    }
}
