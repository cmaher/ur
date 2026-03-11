use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use tracing::info;

use ur_config::{Config, ProjectConfig};

/// Manages a pool of pre-cloned git repositories per project.
///
/// Directory layout: `$WORKSPACE/pool/<project-key>/<slot-index>/`
///
/// Slots are acquired for agent processes and released when processes stop.
/// In-memory tracking only — state is lost on restart.
#[derive(Clone)]
pub struct RepoPoolManager {
    /// Workspace root path (contains `pool/` subdirectory).
    workspace: PathBuf,
    /// Project configs keyed by project key.
    projects: HashMap<String, ProjectConfig>,
    /// Set of slot paths currently in use by running agents.
    in_use: Arc<RwLock<HashSet<PathBuf>>>,
}

impl RepoPoolManager {
    pub fn new(config: &Config) -> Self {
        Self {
            workspace: config.workspace.clone(),
            projects: config.projects.clone(),
            in_use: Arc::new(RwLock::new(HashSet::new())),
        }
    }

    /// Root directory for all pool slots: `$WORKSPACE/pool/`.
    fn pool_root(&self) -> PathBuf {
        self.workspace.join("pool")
    }

    /// Directory for a specific project's pool: `$WORKSPACE/pool/<project-key>/`.
    fn project_pool_dir(&self, project_key: &str) -> PathBuf {
        self.pool_root().join(project_key)
    }

    /// Full path for a specific slot: `$WORKSPACE/pool/<project-key>/<slot-index>/`.
    fn slot_path(&self, project_key: &str, slot_index: u32) -> PathBuf {
        self.project_pool_dir(project_key).join(slot_index.to_string())
    }

    /// Acquire a repo slot for the given project.
    ///
    /// 1. Looks up the project in config.
    /// 2. Scans existing slots for one not in use.
    /// 3. If found, resets it to origin/master and marks it in-use.
    /// 4. If none available, clones a new slot (if under pool_limit).
    ///
    /// Returns the host path to the acquired slot directory.
    pub async fn acquire(&self, project_key: &str) -> Result<PathBuf, String> {
        let project = self
            .projects
            .get(project_key)
            .ok_or_else(|| format!("unknown project: {project_key}"))?;

        let pool_dir = self.project_pool_dir(project_key);

        // Scan existing slots
        let existing_slots = self.scan_slots(&pool_dir).await;

        // Find an available (not in-use) slot
        let available_slot = {
            let in_use = self.in_use.read().expect("pool lock poisoned");
            existing_slots
                .iter()
                .find(|idx| !in_use.contains(&self.slot_path(project_key, **idx)))
                .copied()
        };

        if let Some(slot_index) = available_slot {
            let path = self.slot_path(project_key, slot_index);
            info!(project_key, slot_index, path = %path.display(), "resetting existing pool slot");
            self.reset_slot(&path).await?;
            self.mark_in_use(&path);
            return Ok(path);
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
        let path = self.slot_path(project_key, next_index);

        info!(
            project_key,
            slot_index = next_index,
            repo = %project.repo,
            path = %path.display(),
            "cloning new pool slot"
        );

        self.clone_slot(&project.repo, &path).await?;
        self.mark_in_use(&path);

        Ok(path)
    }

    /// Release a previously acquired slot, resetting it to a clean state.
    ///
    /// Fetches, checks out master, resets to origin/master, and cleans.
    pub async fn release(&self, slot_path: &Path) -> Result<(), String> {
        info!(path = %slot_path.display(), "releasing pool slot");
        self.reset_slot(slot_path).await?;
        self.mark_available(slot_path);
        Ok(())
    }

    /// Scan the project pool directory for existing slot indices.
    /// Returns a sorted vec of slot indices found on disk.
    async fn scan_slots(&self, pool_dir: &Path) -> Vec<u32> {
        let mut slots = Vec::new();
        let Ok(mut entries) = tokio::fs::read_dir(pool_dir).await else {
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

    /// Clone a repo into a new slot directory.
    async fn clone_slot(&self, repo_url: &str, slot_path: &Path) -> Result<(), String> {
        if let Some(parent) = slot_path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| format!("failed to create pool directory: {e}"))?;
        }

        let output = tokio::process::Command::new("git")
            .args(["clone", repo_url, &slot_path.to_string_lossy()])
            .output()
            .await
            .map_err(|e| format!("failed to run git clone: {e}"))?;

        if !output.status.success() {
            return Err(format!(
                "git clone failed: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }

        Ok(())
    }

    /// Reset an existing slot to a clean origin/master state.
    ///
    /// Runs: `git fetch origin && git checkout master && git reset --hard origin/master && git clean -fd`
    async fn reset_slot(&self, slot_path: &Path) -> Result<(), String> {
        let commands: &[&[&str]] = &[
            &["fetch", "origin"],
            &["checkout", "master"],
            &["reset", "--hard", "origin/master"],
            &["clean", "-fd"],
        ];

        for args in commands {
            let output = tokio::process::Command::new("git")
                .args(*args)
                .current_dir(slot_path)
                .output()
                .await
                .map_err(|e| format!("failed to run git {}: {e}", args[0]))?;

            if !output.status.success() {
                return Err(format!(
                    "git {} failed in {}: {}",
                    args.join(" "),
                    slot_path.display(),
                    String::from_utf8_lossy(&output.stderr)
                ));
            }
        }

        Ok(())
    }

    /// Mark a slot path as in-use.
    fn mark_in_use(&self, slot_path: &Path) {
        let mut in_use = self.in_use.write().expect("pool lock poisoned");
        in_use.insert(slot_path.to_path_buf());
    }

    /// Mark a slot path as available (no longer in-use).
    fn mark_available(&self, slot_path: &Path) {
        let mut in_use = self.in_use.write().expect("pool lock poisoned");
        in_use.remove(slot_path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Create a RepoPoolManager backed by a temp directory with a fake project config.
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
            },
        );
        let mgr = RepoPoolManager {
            workspace: workspace.clone(),
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

        assert_eq!(mgr.pool_root(), workspace.join("pool"));
        assert_eq!(
            mgr.project_pool_dir("myproj"),
            workspace.join("pool").join("myproj")
        );
        assert_eq!(
            mgr.slot_path("myproj", 3),
            workspace.join("pool").join("myproj").join("3")
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

        // Mark slots 0 and 1 as in-use
        mgr.mark_in_use(&slot0);
        mgr.mark_in_use(&slot1);

        // Acquire should try slot 2, which will fail on git reset (expected in
        // unit tests — the important thing is it selects the right slot).
        // We test the selection logic by checking what the error says.
        let result = mgr.acquire("testproj").await;
        // The git reset will fail because these aren't real git repos,
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

        // No slots exist on disk. Acquire should attempt git clone into slot 0.
        // The clone will fail (no real repo URL), but we verify the correct path is targeted.
        let result = mgr.acquire("testproj").await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        // Should be a git clone error (not "pool limit" or "unknown project")
        assert!(
            err.contains("git clone failed") || err.contains("failed to run git clone"),
            "expected clone error, got: {err}"
        );
        // The slot should NOT be marked in-use since clone failed
        let expected_slot = workspace.join("pool").join("testproj").join("0");
        assert!(!mgr.in_use.read().unwrap().contains(&expected_slot));
    }
}
