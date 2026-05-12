use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use tonic::{Request, Response, Status};
use tracing::{info, warn};

use ur_rpc::proto::builder_pool::builder_pool_service_server::BuilderPoolService;
use ur_rpc::proto::builder_pool::{
    CheckoutBranchRequest, CheckoutBranchResponse, CleanSlotRequest, CleanSlotResponse,
    PrepareNewSlotRequest, PrepareNewSlotResponse, PrepareSharedSlotRequest,
    PrepareSharedSlotResponse, RecycleSlotRequest, RecycleSlotResponse, ScanSlotsRequest,
    ScanSlotsResponse,
};

/// Handles BuilderPoolService RPCs: pool slot lifecycle (clone, reset, clean) and
/// branch checkout. Runs natively on the host with direct filesystem and git access.
#[derive(Clone)]
pub struct BuilderPoolHandler {
    /// Host-side workspace root — pool lives at `<workspace>/pool/<project>/<slot>/`.
    pub workspace: PathBuf,
    /// Host-side config directory — local overlay files live at
    /// `<config_dir>/projects/<project>/local/`.
    pub config_dir: PathBuf,
}

impl BuilderPoolHandler {
    /// Path to the project pool directory: `<workspace>/pool/<project>/`.
    fn project_pool_dir(&self, project_key: &str) -> PathBuf {
        self.workspace.join("pool").join(project_key)
    }

    /// Path to a specific slot: `<workspace>/pool/<project>/<slot_name>/`.
    fn slot_path(&self, project_key: &str, slot_name: &str) -> PathBuf {
        self.project_pool_dir(project_key).join(slot_name)
    }

    /// Scan the project pool directory for numeric slot indices.
    /// Returns a sorted vec of all `u32` slot names found on disk.
    async fn scan_slot_indices(&self, project_key: &str) -> Vec<u32> {
        let pool_dir = self.project_pool_dir(project_key);
        let mut indices = Vec::new();
        let Ok(mut entries) = tokio::fs::read_dir(&pool_dir).await else {
            return indices;
        };
        while let Ok(Some(entry)) = entries.next_entry().await {
            if let Some(name) = entry.file_name().to_str()
                && let Ok(idx) = name.parse::<u32>()
                && entry.path().is_dir()
            {
                indices.push(idx);
            }
        }
        indices.sort();
        indices
    }

    /// Run a git command in the given working directory.
    /// Returns `Err` with combined stderr output on failure.
    async fn git(&self, args: &[&str], working_dir: &Path) -> Result<(), String> {
        let output = tokio::process::Command::new("git")
            .args(args)
            .current_dir(working_dir)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await
            .map_err(|e| format!("failed to spawn git: {e}"))?;

        if output.status.success() {
            Ok(())
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stdout = String::from_utf8_lossy(&output.stdout);
            Err(format!(
                "git {} failed (exit {:?}): {}{}",
                args.join(" "),
                output.status.code(),
                stderr,
                stdout
            ))
        }
    }

    /// Clone a repo into a new slot directory.
    ///
    /// Creates the parent directory, clones the repo as `slot_name` inside it,
    /// initializes submodules, and trusts mise if present.
    async fn clone_slot(
        &self,
        project_key: &str,
        slot_name: &str,
        repo_url: &str,
    ) -> Result<(), String> {
        let parent = self.project_pool_dir(project_key);
        tokio::fs::create_dir_all(&parent)
            .await
            .map_err(|e| format!("failed to create pool dir {}: {e}", parent.display()))?;

        // Clone using slot_name as the destination path (relative to parent).
        self.git(
            &[
                "clone",
                "--filter=blob:none",
                "--no-tags",
                "--single-branch",
                repo_url,
                slot_name,
            ],
            &parent,
        )
        .await
        .map_err(|e| format!("git clone failed for {repo_url}: {e}"))?;

        let slot = self.slot_path(project_key, slot_name);
        self.init_submodules(&slot).await?;
        self.trust_mise(&slot).await;

        Ok(())
    }

    /// Remove a slot and re-clone it from scratch.
    ///
    /// Retries `rm -rf` up to 3 times with a 1s delay to handle macOS Spotlight
    /// holding file locks that cause transient "directory not empty" errors.
    async fn reclone_slot(
        &self,
        project_key: &str,
        slot_name: &str,
        repo_url: &str,
    ) -> Result<(), String> {
        let slot = self.slot_path(project_key, slot_name);
        let slot_display = slot.display().to_string();

        ur_utils::retry(3, Duration::from_secs(1), "rm -rf slot", || {
            let slot = slot.clone();
            async move {
                tokio::fs::remove_dir_all(&slot)
                    .await
                    .map_err(|e| format!("rm -rf {} failed: {e}", slot.display()))
            }
        })
        .await
        .map_err(|e| format!("failed to remove corrupted slot {slot_display}: {e}"))?;

        self.clone_slot(project_key, slot_name, repo_url)
            .await
            .map_err(|e| format!("reclone failed for slot {slot_display}: {e}"))
    }

    /// Reset an existing slot to a clean state: fetch, checkout master,
    /// reset --hard origin/master, clean -fdx, submodule update.
    async fn reset_slot(&self, project_key: &str, slot_name: &str) -> Result<(), String> {
        let slot = self.slot_path(project_key, slot_name);

        self.git(&["fetch", "--no-tags", "origin"], &slot)
            .await
            .map_err(|e| format!("git fetch failed in {}: {e}", slot.display()))?;

        self.git(&["checkout", "master"], &slot)
            .await
            .map_err(|e| format!("git checkout master failed in {}: {e}", slot.display()))?;

        self.git(&["reset", "--hard", "origin/master"], &slot)
            .await
            .map_err(|e| {
                format!(
                    "git reset --hard origin/master failed in {}: {e}",
                    slot.display()
                )
            })?;

        self.git(&["clean", "-fdx"], &slot)
            .await
            .map_err(|e| format!("git clean -fdx failed in {}: {e}", slot.display()))?;

        self.init_submodules(&slot).await?;

        Ok(())
    }

    /// Refresh a shared slot: fetch + reset --hard origin/HEAD + submodule update.
    async fn refresh_shared_slot(&self, slot: &Path) -> Result<(), String> {
        self.git(&["fetch", "--no-tags", "origin"], slot)
            .await
            .map_err(|e| format!("git fetch failed in {}: {e}", slot.display()))?;

        self.git(&["reset", "--hard", "origin/HEAD"], slot)
            .await
            .map_err(|e| {
                format!(
                    "git reset --hard origin/HEAD failed in {}: {e}",
                    slot.display()
                )
            })?;

        self.init_submodules(slot).await?;

        Ok(())
    }

    /// Initialize/update git submodules if `.gitmodules` exists.
    async fn init_submodules(&self, slot: &Path) -> Result<(), String> {
        let gitmodules = slot.join(".gitmodules");
        if !tokio::fs::try_exists(&gitmodules).await.unwrap_or(false) {
            return Ok(());
        }

        info!(path = %slot.display(), "initializing git submodules");
        self.git(
            &[
                "submodule",
                "update",
                "--init",
                "--recursive",
                "--depth",
                "1",
            ],
            slot,
        )
        .await
        .map_err(|e| {
            format!(
                "git submodule update --init --recursive failed in {}: {e}",
                slot.display()
            )
        })
    }

    /// Trust mise configuration if `mise.toml` exists. Failure is non-fatal.
    async fn trust_mise(&self, slot: &Path) {
        let mise_toml = slot.join("mise.toml");
        if !tokio::fs::try_exists(&mise_toml).await.unwrap_or(false) {
            return;
        }

        info!(path = %slot.display(), "trusting mise.toml in slot");
        if let Err(e) = tokio::process::Command::new("mise")
            .args(["trust"])
            .current_dir(slot)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await
        {
            warn!(
                path = %slot.display(),
                error = %e,
                "mise trust failed (mise may not be installed)"
            );
        }
    }

    /// Copy local overlay files from `<config_dir>/projects/<project>/local/` into the slot.
    /// No-op if the source directory does not exist.
    fn apply_local_files(&self, project_key: &str, slot: &Path) -> Result<(), String> {
        let source = self
            .config_dir
            .join("projects")
            .join(project_key)
            .join("local");

        if !source.is_dir() {
            return Ok(());
        }

        copy_dir_recursive(&source, slot)
    }
}

/// Recursively copy all files and directories from `src` into `dst`.
/// Creates intermediate directories as needed; overwrites existing files.
fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<(), String> {
    let entries = std::fs::read_dir(src)
        .map_err(|e| format!("failed to read directory {}: {e}", src.display()))?;

    for entry in entries {
        let entry = entry.map_err(|e| format!("failed to read entry in {}: {e}", src.display()))?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());

        if src_path.is_dir() {
            std::fs::create_dir_all(&dst_path)
                .map_err(|e| format!("failed to create directory {}: {e}", dst_path.display()))?;
            copy_dir_recursive(&src_path, &dst_path)?;
        } else {
            copy_file(&src_path, &dst_path)?;
        }
    }

    Ok(())
}

/// Copy a single file, creating parent directories as needed.
fn copy_file(src: &Path, dst: &Path) -> Result<(), String> {
    if let Some(parent) = dst.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            format!(
                "failed to create parent directory {}: {e}",
                parent.display()
            )
        })?;
    }
    std::fs::copy(src, dst)
        .map_err(|e| format!("failed to copy {} to {}: {e}", src.display(), dst.display()))?;
    Ok(())
}

/// Map an internal `String` error to a gRPC `Internal` status.
fn internal(msg: impl Into<String>) -> Status {
    Status::internal(msg.into())
}

#[tonic::async_trait]
impl BuilderPoolService for BuilderPoolHandler {
    async fn scan_slots(
        &self,
        req: Request<ScanSlotsRequest>,
    ) -> Result<Response<ScanSlotsResponse>, Status> {
        let req = req.into_inner();
        info!(project_key = %req.project_key, "ScanSlots request received");

        let indices = self.scan_slot_indices(&req.project_key).await;
        Ok(Response::new(ScanSlotsResponse {
            slot_indices: indices,
        }))
    }

    async fn prepare_new_slot(
        &self,
        req: Request<PrepareNewSlotRequest>,
    ) -> Result<Response<PrepareNewSlotResponse>, Status> {
        let req = req.into_inner();
        info!(
            project_key = %req.project_key,
            slot_name = %req.slot_name,
            repo_url = %req.repo_url,
            "PrepareNewSlot request received"
        );

        self.clone_slot(&req.project_key, &req.slot_name, &req.repo_url)
            .await
            .map_err(internal)?;

        self.apply_local_files(
            &req.project_key,
            &self.slot_path(&req.project_key, &req.slot_name),
        )
        .map_err(internal)?;

        let host_path = self
            .slot_path(&req.project_key, &req.slot_name)
            .display()
            .to_string();
        Ok(Response::new(PrepareNewSlotResponse { host_path }))
    }

    async fn recycle_slot(
        &self,
        req: Request<RecycleSlotRequest>,
    ) -> Result<Response<RecycleSlotResponse>, Status> {
        let req = req.into_inner();
        info!(
            project_key = %req.project_key,
            slot_name = %req.slot_name,
            "RecycleSlot request received"
        );

        // Attempt reset; on failure reclone from scratch.
        if let Err(reset_err) = self.reset_slot(&req.project_key, &req.slot_name).await {
            warn!(
                project_key = %req.project_key,
                slot_name = %req.slot_name,
                error = %reset_err,
                "reset failed, re-cloning slot"
            );
            self.reclone_slot(&req.project_key, &req.slot_name, &req.repo_url)
                .await
                .map_err(internal)?;
        }

        self.apply_local_files(
            &req.project_key,
            &self.slot_path(&req.project_key, &req.slot_name),
        )
        .map_err(internal)?;

        let host_path = self
            .slot_path(&req.project_key, &req.slot_name)
            .display()
            .to_string();
        Ok(Response::new(RecycleSlotResponse { host_path }))
    }

    async fn prepare_shared_slot(
        &self,
        req: Request<PrepareSharedSlotRequest>,
    ) -> Result<Response<PrepareSharedSlotResponse>, Status> {
        let req = req.into_inner();
        let slot_name = "shared";
        info!(
            project_key = %req.project_key,
            slot_name,
            "PrepareSharedSlot request received"
        );

        let slot = self.slot_path(&req.project_key, slot_name);
        let exists = tokio::fs::try_exists(&slot).await.unwrap_or(false);

        if exists {
            info!(project_key = %req.project_key, path = %slot.display(), "refreshing shared slot");
            self.refresh_shared_slot(&slot).await.map_err(internal)?;
        } else {
            info!(
                project_key = %req.project_key,
                repo_url = %req.repo_url,
                path = %slot.display(),
                "cloning shared slot"
            );
            self.clone_slot(&req.project_key, slot_name, &req.repo_url)
                .await
                .map_err(internal)?;
        }

        let host_path = slot.display().to_string();
        Ok(Response::new(PrepareSharedSlotResponse { host_path }))
    }

    async fn checkout_branch(
        &self,
        req: Request<CheckoutBranchRequest>,
    ) -> Result<Response<CheckoutBranchResponse>, Status> {
        let req = req.into_inner();
        let full_branch = format!("{}{}", req.branch_prefix, req.branch_name);
        info!(
            project_key = %req.project_key,
            slot_name = %req.slot_name,
            branch = %full_branch,
            "CheckoutBranch request received"
        );

        let slot = self.slot_path(&req.project_key, &req.slot_name);
        if !slot.is_dir() {
            return Err(Status::not_found(format!(
                "slot not found: {}/{}",
                req.project_key, req.slot_name
            )));
        }

        self.git(&["checkout", "-B", &full_branch], &slot)
            .await
            .map_err(|e| {
                internal(format!(
                    "git checkout -B {full_branch} failed in {}: {e}",
                    slot.display()
                ))
            })?;

        Ok(Response::new(CheckoutBranchResponse {}))
    }

    async fn clean_slot(
        &self,
        req: Request<CleanSlotRequest>,
    ) -> Result<Response<CleanSlotResponse>, Status> {
        let req = req.into_inner();
        info!(
            project_key = %req.project_key,
            slot_name = %req.slot_name,
            "CleanSlot request received"
        );

        let slot = self.slot_path(&req.project_key, &req.slot_name);
        if !slot.is_dir() {
            return Err(Status::not_found(format!(
                "slot not found: {}/{}",
                req.project_key, req.slot_name
            )));
        }

        // Reset without applying local files (clean state only).
        self.reset_slot(&req.project_key, &req.slot_name)
            .await
            .map_err(internal)?;

        Ok(Response::new(CleanSlotResponse {}))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn make_handler(tmp: &Path) -> BuilderPoolHandler {
        BuilderPoolHandler {
            workspace: tmp.join("workspace"),
            config_dir: tmp.join("config"),
        }
    }

    // ── scan_slots ────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn scan_slots_empty_when_dir_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let h = make_handler(tmp.path());
        let indices = h.scan_slot_indices("myproj").await;
        assert!(indices.is_empty());
    }

    #[tokio::test]
    async fn scan_slots_finds_numeric_dirs_sorted() {
        let tmp = tempfile::tempdir().unwrap();
        let h = make_handler(tmp.path());
        let pool = h.project_pool_dir("myproj");
        std::fs::create_dir_all(pool.join("0")).unwrap();
        std::fs::create_dir_all(pool.join("3")).unwrap();
        std::fs::create_dir_all(pool.join("1")).unwrap();
        // Non-numeric — should be ignored.
        std::fs::create_dir_all(pool.join("shared")).unwrap();

        let indices = h.scan_slot_indices("myproj").await;
        assert_eq!(indices, vec![0, 1, 3]);
    }

    // ── apply_local_files ─────────────────────────────────────────────────────

    #[tokio::test]
    async fn apply_local_files_noop_when_source_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let h = make_handler(tmp.path());
        let slot = tmp.path().join("slot");
        std::fs::create_dir_all(&slot).unwrap();
        assert!(h.apply_local_files("proj", &slot).is_ok());
    }

    #[tokio::test]
    async fn apply_local_files_copies_files() {
        let tmp = tempfile::tempdir().unwrap();
        let h = make_handler(tmp.path());

        // Create source local directory.
        let src = h.config_dir.join("projects").join("proj").join("local");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("hello.txt"), "world").unwrap();
        let nested_dir = src.join("sub");
        std::fs::create_dir_all(&nested_dir).unwrap();
        std::fs::write(nested_dir.join("nested.txt"), "nested").unwrap();

        let slot = tmp.path().join("slot");
        std::fs::create_dir_all(&slot).unwrap();

        h.apply_local_files("proj", &slot).unwrap();

        assert_eq!(
            std::fs::read_to_string(slot.join("hello.txt")).unwrap(),
            "world"
        );
        assert_eq!(
            std::fs::read_to_string(slot.join("sub").join("nested.txt")).unwrap(),
            "nested"
        );
    }

    // ── git operations with a local bare repo ────────────────────────────────

    /// Create a bare git repo at `bare_path` with a single commit on `master`.
    fn init_bare_repo(bare_path: &Path) {
        // Init bare repo.
        std::process::Command::new("git")
            .args(["init", "--bare", "--initial-branch=master"])
            .arg(bare_path)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap();

        // Create a working clone, commit a file, then push to the bare repo.
        let tmp_work = tempfile::tempdir().unwrap();
        let work_path = tmp_work.path();
        std::process::Command::new("git")
            .args(["clone", &bare_path.display().to_string(), "."])
            .current_dir(work_path)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap();
        std::fs::write(work_path.join("README.md"), "hello").unwrap();
        std::process::Command::new("git")
            .args(["-c", "user.email=test@test", "-c", "user.name=Test"])
            .args(["add", "README.md"])
            .current_dir(work_path)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap();
        std::process::Command::new("git")
            .args(["-c", "user.email=test@test", "-c", "user.name=Test"])
            .args(["commit", "-m", "init"])
            .current_dir(work_path)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap();
        std::process::Command::new("git")
            .args(["push", "origin", "master"])
            .current_dir(work_path)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap();
    }

    #[tokio::test]
    async fn prepare_new_slot_clones_repo() {
        let tmp = tempfile::tempdir().unwrap();
        let bare = tmp.path().join("bare.git");
        init_bare_repo(&bare);

        let h = make_handler(tmp.path());
        let repo_url = bare.display().to_string();

        h.clone_slot("proj", "0", &repo_url).await.unwrap();

        let slot = h.slot_path("proj", "0");
        assert!(
            slot.join("README.md").exists(),
            "README.md should be cloned"
        );
    }

    #[tokio::test]
    async fn scan_slots_after_clone() {
        let tmp = tempfile::tempdir().unwrap();
        let bare = tmp.path().join("bare.git");
        init_bare_repo(&bare);

        let h = make_handler(tmp.path());
        let repo_url = bare.display().to_string();

        h.clone_slot("proj", "0", &repo_url).await.unwrap();
        h.clone_slot("proj", "1", &repo_url).await.unwrap();

        let indices = h.scan_slot_indices("proj").await;
        assert_eq!(indices, vec![0, 1]);
    }

    #[tokio::test]
    async fn reset_slot_restores_clean_state() {
        let tmp = tempfile::tempdir().unwrap();
        let bare = tmp.path().join("bare.git");
        init_bare_repo(&bare);

        let h = make_handler(tmp.path());
        let repo_url = bare.display().to_string();

        h.clone_slot("proj", "0", &repo_url).await.unwrap();

        // Dirty the slot with an untracked file and a modified tracked file.
        let slot = h.slot_path("proj", "0");
        std::fs::write(slot.join("dirty.txt"), "dirty").unwrap();
        std::fs::write(slot.join("README.md"), "modified").unwrap();

        h.reset_slot("proj", "0").await.unwrap();

        assert!(
            !slot.join("dirty.txt").exists(),
            "untracked file should be removed after reset"
        );
        let readme = std::fs::read_to_string(slot.join("README.md")).unwrap();
        assert_eq!(readme, "hello", "tracked file should be restored");
    }

    #[tokio::test]
    async fn reclone_slot_replaces_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let bare = tmp.path().join("bare.git");
        init_bare_repo(&bare);

        let h = make_handler(tmp.path());
        let repo_url = bare.display().to_string();

        h.clone_slot("proj", "0", &repo_url).await.unwrap();

        // Corrupt the slot by removing .git so reset would fail.
        let slot = h.slot_path("proj", "0");
        std::fs::remove_dir_all(slot.join(".git")).unwrap();

        // reclone should recover.
        h.reclone_slot("proj", "0", &repo_url).await.unwrap();
        assert!(
            slot.join("README.md").exists(),
            "README.md should exist after reclone"
        );
    }

    #[tokio::test]
    async fn checkout_branch_creates_branch() {
        let tmp = tempfile::tempdir().unwrap();
        let bare = tmp.path().join("bare.git");
        init_bare_repo(&bare);

        let h = make_handler(tmp.path());
        let repo_url = bare.display().to_string();

        h.clone_slot("proj", "0", &repo_url).await.unwrap();
        let slot = h.slot_path("proj", "0");

        h.git(&["checkout", "-B", "worker-abc"], &slot)
            .await
            .unwrap();

        let output = std::process::Command::new("git")
            .args(["rev-parse", "--abbrev-ref", "HEAD"])
            .current_dir(&slot)
            .output()
            .unwrap();
        let branch = String::from_utf8_lossy(&output.stdout);
        assert_eq!(branch.trim(), "worker-abc");
    }

    #[tokio::test]
    async fn clean_slot_not_found_returns_error() {
        let tmp = tempfile::tempdir().unwrap();
        let h = make_handler(tmp.path());

        let req = Request::new(CleanSlotRequest {
            project_key: "proj".into(),
            slot_name: "99".into(),
        });
        let result = h.clean_slot(req).await;
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().code(), tonic::Code::NotFound);
    }

    #[tokio::test]
    async fn checkout_branch_not_found_returns_error() {
        let tmp = tempfile::tempdir().unwrap();
        let h = make_handler(tmp.path());

        let req = Request::new(CheckoutBranchRequest {
            project_key: "proj".into(),
            slot_name: "99".into(),
            branch_prefix: "workers/".into(),
            branch_name: "abc".into(),
        });
        let result = h.checkout_branch(req).await;
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().code(), tonic::Code::NotFound);
    }

    #[tokio::test]
    async fn prepare_shared_slot_clones_then_refreshes() {
        let tmp = tempfile::tempdir().unwrap();
        let bare = tmp.path().join("bare.git");
        init_bare_repo(&bare);

        let h = make_handler(tmp.path());
        let repo_url = bare.display().to_string();

        // First call — directory missing — should clone.
        let req1 = Request::new(PrepareSharedSlotRequest {
            project_key: "proj".into(),
            repo_url: repo_url.clone(),
        });
        let resp1 = h.prepare_shared_slot(req1).await.unwrap().into_inner();
        let expected_path = h.slot_path("proj", "shared");
        assert_eq!(resp1.host_path, expected_path.display().to_string());
        assert!(expected_path.join("README.md").exists());

        // Modify a tracked file to verify reset restores it.
        std::fs::write(expected_path.join("README.md"), "modified").unwrap();

        // Second call — directory exists — should refresh (fetch + reset).
        let req2 = Request::new(PrepareSharedSlotRequest {
            project_key: "proj".into(),
            repo_url: repo_url.clone(),
        });
        h.prepare_shared_slot(req2).await.unwrap();

        // Tracked file should be restored to original content after reset --hard.
        let readme = std::fs::read_to_string(expected_path.join("README.md")).unwrap();
        assert_eq!(readme, "hello", "tracked file restored after refresh");
    }

    #[tokio::test]
    async fn apply_local_files_overwrites_existing() {
        let tmp = tempfile::tempdir().unwrap();
        let h = make_handler(tmp.path());

        let src = h.config_dir.join("projects").join("proj").join("local");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("config.txt"), "new-value").unwrap();

        let slot = tmp.path().join("slot");
        std::fs::create_dir_all(&slot).unwrap();
        std::fs::write(slot.join("config.txt"), "old-value").unwrap();

        h.apply_local_files("proj", &slot).unwrap();
        assert_eq!(
            std::fs::read_to_string(slot.join("config.txt")).unwrap(),
            "new-value"
        );
    }

    #[tokio::test]
    async fn prepare_new_slot_applies_local_files() {
        let tmp = tempfile::tempdir().unwrap();
        let bare = tmp.path().join("bare.git");
        init_bare_repo(&bare);

        let h = make_handler(tmp.path());
        let repo_url = bare.display().to_string();

        // Create a local overlay file.
        let src = h.config_dir.join("projects").join("proj").join("local");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("overlay.txt"), "from-local").unwrap();

        let req = Request::new(PrepareNewSlotRequest {
            project_key: "proj".into(),
            slot_name: "0".into(),
            repo_url,
        });
        h.prepare_new_slot(req).await.unwrap();

        let slot = h.slot_path("proj", "0");
        assert_eq!(
            std::fs::read_to_string(slot.join("overlay.txt")).unwrap(),
            "from-local"
        );
    }

    #[tokio::test]
    async fn recycle_slot_resets_and_applies_local_files() {
        let tmp = tempfile::tempdir().unwrap();
        let bare = tmp.path().join("bare.git");
        init_bare_repo(&bare);

        let h = make_handler(tmp.path());
        let repo_url = bare.display().to_string();

        h.clone_slot("proj", "0", &repo_url).await.unwrap();
        let slot = h.slot_path("proj", "0");

        // Dirty the slot.
        std::fs::write(slot.join("untracked.txt"), "garbage").unwrap();

        // Set up local overlay.
        let src = h.config_dir.join("projects").join("proj").join("local");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("local.txt"), "local-content").unwrap();

        let req = Request::new(RecycleSlotRequest {
            project_key: "proj".into(),
            slot_name: "0".into(),
            repo_url,
        });
        h.recycle_slot(req).await.unwrap();

        assert!(
            !slot.join("untracked.txt").exists(),
            "untracked file removed after recycle"
        );
        assert_eq!(
            std::fs::read_to_string(slot.join("local.txt")).unwrap(),
            "local-content",
            "local overlay applied after recycle"
        );
    }

    #[tokio::test]
    async fn scan_slots_rpc_returns_sorted_indices() {
        let tmp = tempfile::tempdir().unwrap();
        let h = make_handler(tmp.path());
        let pool = h.project_pool_dir("proj");
        std::fs::create_dir_all(pool.join("2")).unwrap();
        std::fs::create_dir_all(pool.join("0")).unwrap();
        std::fs::create_dir_all(pool.join("5")).unwrap();

        let req = Request::new(ScanSlotsRequest {
            project_key: "proj".into(),
        });
        let resp = h.scan_slots(req).await.unwrap().into_inner();
        assert_eq!(resp.slot_indices, vec![0, 2, 5]);
    }

    #[tokio::test]
    async fn clean_slot_resets_without_local_files() {
        let tmp = tempfile::tempdir().unwrap();
        let bare = tmp.path().join("bare.git");
        init_bare_repo(&bare);

        let h = make_handler(tmp.path());
        let repo_url = bare.display().to_string();

        h.clone_slot("proj", "0", &repo_url).await.unwrap();
        let slot = h.slot_path("proj", "0");

        // Dirty the slot.
        std::fs::write(slot.join("junk.txt"), "junk").unwrap();

        // Set up local overlay — it should NOT be applied by clean_slot.
        let src = h.config_dir.join("projects").join("proj").join("local");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("local.txt"), "should-not-appear").unwrap();

        let req = Request::new(CleanSlotRequest {
            project_key: "proj".into(),
            slot_name: "0".into(),
        });
        h.clean_slot(req).await.unwrap();

        assert!(
            !slot.join("junk.txt").exists(),
            "junk file removed after clean"
        );
        assert!(
            !slot.join("local.txt").exists(),
            "local overlay must NOT be applied by clean_slot"
        );
    }

    // ── copy_dir_recursive ────────────────────────────────────────────────────

    #[test]
    fn copy_dir_recursive_nested() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("src");
        let dst = tmp.path().join("dst");
        std::fs::create_dir_all(src.join("a").join("b")).unwrap();
        std::fs::write(src.join("a").join("b").join("f.txt"), "deep").unwrap();
        std::fs::create_dir_all(&dst).unwrap();

        copy_dir_recursive(&src, &dst).unwrap();
        assert_eq!(
            std::fs::read_to_string(dst.join("a").join("b").join("f.txt")).unwrap(),
            "deep"
        );
    }
}
