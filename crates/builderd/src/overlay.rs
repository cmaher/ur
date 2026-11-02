use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;

use tracing::info;

/// Directory inside a slot's `.git/` holding the overlay state for that slot.
const OVERLAY_STATE_DIR: &str = "ur-overlay";
/// Subdirectory of the overlay state mirroring the overlay files at their slot-relative paths.
const OVERLAY_FILES_DIR: &str = "files";
/// File listing the tracked paths that were pinned with `skip-worktree` on the last reveal.
const OVERLAY_PINNED_FILE: &str = "pinned";

/// Host commands whose execution inside a pool slot is bracketed by conceal/reveal,
/// so that git never observes local overlay files. `gh` is included because
/// commands like `gh pr checkout` drive git underneath.
pub const OVERLAY_BRACKETED_COMMANDS: &[&str] = &["git", "gh"];

/// Proof that a slot's overlays are concealed for one in-flight command. Hand it back
/// to [`LocalOverlayManager::reveal_after`] once the command finishes.
pub struct OverlayLease {
    pub slot: PathBuf,
}

/// Manages local overlay files (`<config_dir>/projects/<project>/local/`) in pool slots.
///
/// Overlays are copied into the slot so the worker reads them, and a copy is kept in
/// `<slot>/.git/ur-overlay/files/`. Every worker git command run in the slot is bracketed:
/// before it runs the overlays are concealed (worker edits captured into the copy, tracked
/// paths restored to their index version, untracked overlays removed), and after it exits
/// they are revealed again. Git therefore never sees an overlay — not in `status`, not in
/// `add`, and not as a local change that blocks `checkout`, `switch`, `rebase` or `merge`.
#[derive(Clone)]
pub struct LocalOverlayManager {
    /// Host-side workspace root — pool lives at `<workspace>/pool/<project>/<slot>/`.
    pub workspace: PathBuf,
    /// Host-side config directory — local overlay files live at
    /// `<config_dir>/projects/<project>/local/`.
    pub config_dir: PathBuf,
    /// Per-slot count of in-flight bracketed commands. The first command to enter
    /// conceals, the last to leave reveals. The outer std::sync::Mutex is held only to
    /// look up or insert the per-slot Arc; it is never held across await points.
    pub active: Arc<std::sync::Mutex<HashMap<PathBuf, Arc<tokio::sync::Mutex<usize>>>>>,
}

impl LocalOverlayManager {
    /// Install the project's local overlay files into `slot` and reveal them.
    ///
    /// Replaces any overlay state left from a previous acquire. No-op (beyond clearing
    /// stale state) if the project has no `local/` directory.
    pub async fn apply(&self, project_key: &str, slot: &Path) -> Result<(), String> {
        self.discard(slot)?;

        let source = self
            .config_dir
            .join("projects")
            .join(project_key)
            .join("local");
        if !source.is_dir() {
            return Ok(());
        }

        info!(
            project_key,
            source = %source.display(),
            slot = %slot.display(),
            "applying local overlay files"
        );

        let files_dir = files_dir(slot);
        std::fs::create_dir_all(&files_dir).map_err(|e| {
            format!(
                "failed to create overlay state {}: {e}",
                files_dir.display()
            )
        })?;
        let mut stashed = Vec::new();
        copy_dir_recursive_collecting(&source, &files_dir, &files_dir, &mut stashed)?;

        self.reveal(slot).await
    }

    /// Remove a slot's overlay state so nothing is re-applied to it. Does not touch the
    /// working tree; callers reset the slot themselves.
    pub fn discard(&self, slot: &Path) -> Result<(), String> {
        let state = state_dir(slot);
        if !state.exists() {
            return Ok(());
        }
        std::fs::remove_dir_all(&state)
            .map_err(|e| format!("failed to remove overlay state {}: {e}", state.display()))
    }

    /// Conceal the overlays of the pool slot containing `working_dir`, if it has any.
    ///
    /// Returns `None` when `working_dir` is not inside a pool slot or the slot carries no
    /// overlays. A returned lease must be passed to [`Self::reveal_after`].
    pub async fn conceal_for(&self, working_dir: &Path) -> Result<Option<OverlayLease>, String> {
        let Some(slot) = self.overlaid_slot_for(working_dir) else {
            return Ok(None);
        };
        let counter = self.counter(&slot);
        let mut active = counter.lock().await;
        if *active == 0
            && let Err(conceal_err) = self.conceal(&slot).await
        {
            // Put whatever was already hidden back before reporting, so a failed conceal
            // never leaves the worker without its overlays.
            return Err(match self.reveal(&slot).await {
                Ok(()) => conceal_err,
                Err(reveal_err) => {
                    format!("{conceal_err}; restoring overlays also failed: {reveal_err}")
                }
            });
        }
        *active += 1;
        Ok(Some(OverlayLease { slot }))
    }

    /// Release a lease taken by [`Self::conceal_for`], revealing the overlays once no
    /// other bracketed command is running in the slot.
    pub async fn reveal_after(&self, lease: OverlayLease) -> Result<(), String> {
        let counter = self.counter(&lease.slot);
        let mut active = counter.lock().await;
        *active = active.saturating_sub(1);
        if *active == 0 {
            self.reveal(&lease.slot).await?;
        }
        Ok(())
    }

    /// The slot root (`<workspace>/pool/<project>/<slot>`) containing `working_dir`, when
    /// that slot has overlay state.
    fn overlaid_slot_for(&self, working_dir: &Path) -> Option<PathBuf> {
        let pool = self.workspace.join("pool");
        let rel = working_dir.strip_prefix(&pool).ok()?;
        let mut components = rel.components();
        let project = components.next()?;
        let slot_name = components.next()?;
        let slot = pool.join(project).join(slot_name);
        files_dir(&slot).is_dir().then_some(slot)
    }

    fn counter(&self, slot: &Path) -> Arc<tokio::sync::Mutex<usize>> {
        let mut map = self.active.lock().unwrap();
        map.entry(slot.to_path_buf())
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(0)))
            .clone()
    }

    /// Hide every overlay from git: capture worker edits into the overlay copy, unpin,
    /// restore tracked paths to their index version, and remove untracked overlays.
    async fn conceal(&self, slot: &Path) -> Result<(), String> {
        let files_dir = files_dir(slot);
        let rel_paths = list_files(&files_dir)?;

        for rel in &rel_paths {
            let worktree_path = slot.join(rel);
            let is_regular_file = std::fs::symlink_metadata(&worktree_path)
                .map(|m| m.file_type().is_file())
                .unwrap_or(false);
            if is_regular_file {
                copy_file(&worktree_path, &files_dir.join(rel))?;
            }
        }

        let pinned = read_pinned(slot)?;
        if !pinned.is_empty() {
            let mut args = vec!["update-index", "--no-skip-worktree", "--"];
            args.extend(pinned.iter().map(String::as_str));
            self.git(&args, slot).await?;
        }

        let tracked = self.tracked_subset(&rel_paths, slot).await?;
        for rel in rel_paths.iter().filter(|rel| !tracked.contains(*rel)) {
            let path = slot.join(rel);
            if std::fs::symlink_metadata(&path).is_ok() {
                std::fs::remove_file(&path)
                    .map_err(|e| format!("failed to remove overlay {}: {e}", path.display()))?;
            }
        }
        if !tracked.is_empty() {
            let mut args = vec!["checkout", "--"];
            args.extend(tracked.iter().map(String::as_str));
            self.git(&args, slot).await?;
        }

        info!(slot = %slot.display(), count = rel_paths.len(), "concealed local overlays");
        Ok(())
    }

    /// Copy the overlays back into the working tree and pin the tracked ones with
    /// `skip-worktree`, so they stay out of `git status` for anything not bracketed.
    async fn reveal(&self, slot: &Path) -> Result<(), String> {
        let mut placed = Vec::new();
        copy_dir_recursive_collecting(&files_dir(slot), slot, slot, &mut placed)?;

        let tracked = self.tracked_subset(&placed, slot).await?;
        let mut pinned: Vec<String> = placed
            .into_iter()
            .filter(|rel| tracked.contains(rel))
            .collect();
        pinned.sort();
        pinned.dedup();
        if !pinned.is_empty() {
            let mut args = vec!["update-index", "--skip-worktree", "--"];
            args.extend(pinned.iter().map(String::as_str));
            self.git(&args, slot).await?;
        }
        write_pinned(slot, &pinned)?;

        info!(slot = %slot.display(), pinned = ?pinned, "revealed local overlays");
        Ok(())
    }

    /// The subset of `rel_paths` tracked in the slot's git index. A slot that is not a
    /// git repository tracks nothing.
    async fn tracked_subset(
        &self,
        rel_paths: &[String],
        slot: &Path,
    ) -> Result<HashSet<String>, String> {
        if rel_paths.is_empty() || !slot.join(".git").join("HEAD").exists() {
            return Ok(HashSet::new());
        }
        let mut args = vec!["--literal-pathspecs", "ls-files", "-z", "--"];
        args.extend(rel_paths.iter().map(String::as_str));
        let output = self.git(&args, slot).await?;
        Ok(output
            .split('\0')
            .filter(|p| !p.is_empty())
            .map(str::to_string)
            .collect())
    }

    /// Run git in `working_dir` and return its stdout.
    async fn git(&self, args: &[&str], working_dir: &Path) -> Result<String, String> {
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
            Ok(String::from_utf8_lossy(&output.stdout).into_owned())
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            Err(format!(
                "git {} failed in {}: {}",
                args.join(" "),
                working_dir.display(),
                stderr.trim()
            ))
        }
    }
}

fn state_dir(slot: &Path) -> PathBuf {
    slot.join(".git").join(OVERLAY_STATE_DIR)
}

fn files_dir(slot: &Path) -> PathBuf {
    state_dir(slot).join(OVERLAY_FILES_DIR)
}

fn read_pinned(slot: &Path) -> Result<Vec<String>, String> {
    let path = state_dir(slot).join(OVERLAY_PINNED_FILE);
    match std::fs::read_to_string(&path) {
        Ok(content) => Ok(content
            .split('\0')
            .filter(|p| !p.is_empty())
            .map(str::to_string)
            .collect()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(format!("failed to read {}: {e}", path.display())),
    }
}

fn write_pinned(slot: &Path, pinned: &[String]) -> Result<(), String> {
    let path = state_dir(slot).join(OVERLAY_PINNED_FILE);
    std::fs::write(&path, pinned.join("\0"))
        .map_err(|e| format!("failed to write {}: {e}", path.display()))
}

/// All regular files under `dir`, as `/`-separated paths relative to `dir`.
fn list_files(dir: &Path) -> Result<Vec<String>, String> {
    let mut files = Vec::new();
    list_files_into(dir, dir, &mut files)?;
    Ok(files)
}

fn list_files_into(root: &Path, dir: &Path, files: &mut Vec<String>) -> Result<(), String> {
    let entries = std::fs::read_dir(dir)
        .map_err(|e| format!("failed to read directory {}: {e}", dir.display()))?;
    for entry in entries {
        let entry = entry.map_err(|e| format!("failed to read entry in {}: {e}", dir.display()))?;
        let path = entry.path();
        if path.is_dir() {
            list_files_into(root, &path, files)?;
        } else if let Ok(rel) = path.strip_prefix(root) {
            files.push(rel.display().to_string());
        }
    }
    Ok(())
}

/// Recursively copy all files and directories from `src` into `dst`,
/// collecting the relative paths of copied files (relative to `root_dst`) into `collected`.
/// Creates intermediate directories as needed; overwrites existing files.
///
/// When a destination path is a symlink, the symlink is removed first and the overlay
/// replaces it with a regular file. The symlink target path is also recorded in `collected`
/// so the caller can pin both paths with skip-worktree.
pub(crate) fn copy_dir_recursive_collecting(
    src: &Path,
    dst: &Path,
    root_dst: &Path,
    collected: &mut Vec<String>,
) -> Result<(), String> {
    let entries = std::fs::read_dir(src)
        .map_err(|e| format!("failed to read directory {}: {e}", src.display()))?;

    for entry in entries {
        let entry = entry.map_err(|e| format!("failed to read entry in {}: {e}", src.display()))?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());

        if src_path.is_dir() {
            std::fs::create_dir_all(&dst_path)
                .map_err(|e| format!("failed to create directory {}: {e}", dst_path.display()))?;
            copy_dir_recursive_collecting(&src_path, &dst_path, root_dst, collected)?;
        } else {
            replace_symlink_collecting(&dst_path, dst, root_dst, collected)?;
            copy_file(&src_path, &dst_path)?;
            if let Ok(rel) = dst_path.strip_prefix(root_dst) {
                collected.push(rel.display().to_string());
            }
        }
    }

    Ok(())
}

/// If `dst_path` is a symlink, record its target in `collected` (so the caller can
/// pin it with skip-worktree) and remove the symlink so the subsequent copy creates
/// a regular file instead of writing through the link.
fn replace_symlink_collecting(
    dst_path: &Path,
    parent_dir: &Path,
    root_dst: &Path,
    collected: &mut Vec<String>,
) -> Result<(), String> {
    let is_symlink = std::fs::symlink_metadata(dst_path)
        .map(|m| m.file_type().is_symlink())
        .unwrap_or(false);
    if !is_symlink {
        return Ok(());
    }
    if let Ok(target) = std::fs::read_link(dst_path) {
        let target_abs = dst_path.parent().unwrap_or(parent_dir).join(&target);
        if let Ok(rel) = target_abs.strip_prefix(root_dst) {
            collected.push(rel.display().to_string());
        }
    }
    std::fs::remove_file(dst_path)
        .map_err(|e| format!("failed to remove symlink {}: {e}", dst_path.display()))
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

#[cfg(test)]
mod tests {
    use super::*;

    fn make_manager(tmp: &Path) -> LocalOverlayManager {
        LocalOverlayManager {
            workspace: tmp.join("workspace"),
            config_dir: tmp.join("config"),
            active: Arc::new(std::sync::Mutex::new(HashMap::new())),
        }
    }

    fn git_sync(args: &[&str], cwd: &Path) -> String {
        let output = std::process::Command::new("git")
            .args(["-c", "user.email=test@test", "-c", "user.name=Test"])
            .args(args)
            .current_dir(cwd)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).into_owned()
    }

    fn git_succeeds(args: &[&str], cwd: &Path) -> bool {
        std::process::Command::new("git")
            .args(args)
            .current_dir(cwd)
            .output()
            .unwrap()
            .status
            .success()
    }

    /// Create pool slot `proj/0` as a git repo with `README.md` on `master` and a `pr`
    /// branch that changes it.
    fn init_slot(m: &LocalOverlayManager) -> PathBuf {
        let slot = m.workspace.join("pool").join("proj").join("0");
        std::fs::create_dir_all(&slot).unwrap();
        git_sync(&["init", "--initial-branch=master"], &slot);
        std::fs::write(slot.join("README.md"), "base").unwrap();
        git_sync(&["add", "README.md"], &slot);
        git_sync(&["commit", "-m", "base"], &slot);
        git_sync(&["checkout", "-b", "pr"], &slot);
        std::fs::write(slot.join("README.md"), "pr version").unwrap();
        git_sync(&["commit", "-am", "pr"], &slot);
        git_sync(&["checkout", "master"], &slot);
        slot
    }

    fn write_local(m: &LocalOverlayManager, rel: &str, content: &str) {
        let path = m
            .config_dir
            .join("projects")
            .join("proj")
            .join("local")
            .join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }

    fn read(slot: &Path, rel: &str) -> String {
        std::fs::read_to_string(slot.join(rel)).unwrap()
    }

    /// Run `args` the way builderd's exec path does: inside a conceal/reveal bracket.
    async fn bracketed(m: &LocalOverlayManager, slot: &Path, args: &[&str]) -> String {
        let lease = m
            .conceal_for(slot)
            .await
            .unwrap()
            .expect("slot has overlays");
        let out = git_sync(args, slot);
        m.reveal_after(lease).await.unwrap();
        out
    }

    #[tokio::test]
    async fn checkout_changing_overlaid_file_succeeds_when_bracketed() {
        let tmp = tempfile::tempdir().unwrap();
        let m = make_manager(tmp.path());
        let slot = init_slot(&m);
        write_local(&m, "README.md", "override");
        m.apply("proj", &slot).await.unwrap();

        assert!(
            !git_succeeds(&["checkout", "pr"], &slot),
            "unbracketed checkout is blocked by the skip-worktree overlay"
        );

        bracketed(&m, &slot, &["checkout", "pr"]).await;

        assert_eq!(read(&slot, "README.md"), "override");
        assert_eq!(
            git_sync(&["rev-parse", "--abbrev-ref", "HEAD"], &slot).trim(),
            "pr"
        );
        assert_eq!(git_sync(&["show", "HEAD:README.md"], &slot), "pr version");
        assert!(
            git_sync(&["status", "--porcelain"], &slot)
                .trim()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn untracked_overlay_is_invisible_to_bracketed_git() {
        let tmp = tempfile::tempdir().unwrap();
        let m = make_manager(tmp.path());
        let slot = init_slot(&m);
        write_local(&m, ".cargo/config.toml", "[build]");
        m.apply("proj", &slot).await.unwrap();

        let status = bracketed(&m, &slot, &["status", "--porcelain"]).await;
        assert!(
            status.trim().is_empty(),
            "overlay leaked into status: {status}"
        );
        bracketed(&m, &slot, &["add", "-A"]).await;
        assert!(
            git_sync(&["diff", "--cached", "--name-only"], &slot)
                .trim()
                .is_empty()
        );

        assert_eq!(read(&slot, ".cargo/config.toml"), "[build]");
    }

    #[tokio::test]
    async fn worker_edits_to_overlay_survive_bracket() {
        let tmp = tempfile::tempdir().unwrap();
        let m = make_manager(tmp.path());
        let slot = init_slot(&m);
        write_local(&m, "README.md", "override");
        write_local(&m, "notes.txt", "local notes");
        m.apply("proj", &slot).await.unwrap();

        std::fs::write(slot.join("README.md"), "edited by worker").unwrap();
        std::fs::write(slot.join("notes.txt"), "notes edited").unwrap();
        bracketed(&m, &slot, &["status"]).await;

        assert_eq!(read(&slot, "README.md"), "edited by worker");
        assert_eq!(read(&slot, "notes.txt"), "notes edited");
    }

    #[tokio::test]
    async fn overlapping_leases_reveal_only_after_last() {
        let tmp = tempfile::tempdir().unwrap();
        let m = make_manager(tmp.path());
        let slot = init_slot(&m);
        write_local(&m, "README.md", "override");
        m.apply("proj", &slot).await.unwrap();

        let first = m.conceal_for(&slot).await.unwrap().unwrap();
        let second = m.conceal_for(&slot.join("sub")).await.unwrap().unwrap();
        assert_eq!(read(&slot, "README.md"), "base");

        m.reveal_after(first).await.unwrap();
        assert_eq!(
            read(&slot, "README.md"),
            "base",
            "still concealed for second"
        );

        m.reveal_after(second).await.unwrap();
        assert_eq!(read(&slot, "README.md"), "override");
    }

    #[tokio::test]
    async fn conceal_for_ignores_dirs_without_overlays() {
        let tmp = tempfile::tempdir().unwrap();
        let m = make_manager(tmp.path());
        let slot = init_slot(&m);

        assert!(
            m.conceal_for(&slot).await.unwrap().is_none(),
            "no overlay state"
        );
        assert!(
            m.conceal_for(&tmp.path().join("elsewhere"))
                .await
                .unwrap()
                .is_none(),
            "outside the pool"
        );
    }

    #[tokio::test]
    async fn discard_drops_overlay_state() {
        let tmp = tempfile::tempdir().unwrap();
        let m = make_manager(tmp.path());
        let slot = init_slot(&m);
        write_local(&m, "README.md", "override");
        m.apply("proj", &slot).await.unwrap();

        m.discard(&slot).unwrap();
        assert!(!state_dir(&slot).exists());
        assert!(m.conceal_for(&slot).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn apply_replaces_stale_overlay_state() {
        let tmp = tempfile::tempdir().unwrap();
        let m = make_manager(tmp.path());
        let slot = init_slot(&m);
        write_local(&m, "old.txt", "old");
        m.apply("proj", &slot).await.unwrap();

        std::fs::remove_file(m.config_dir.join("projects/proj/local/old.txt")).unwrap();
        write_local(&m, "new.txt", "new");
        m.apply("proj", &slot).await.unwrap();

        assert_eq!(list_files(&files_dir(&slot)).unwrap(), vec!["new.txt"]);
    }

    #[test]
    fn copy_dir_recursive_nested() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("src");
        let dst = tmp.path().join("dst");
        std::fs::create_dir_all(src.join("a").join("b")).unwrap();
        std::fs::write(src.join("a").join("b").join("f.txt"), "deep").unwrap();
        std::fs::create_dir_all(&dst).unwrap();

        let mut collected = Vec::new();
        copy_dir_recursive_collecting(&src, &dst, &dst, &mut collected).unwrap();
        assert_eq!(
            std::fs::read_to_string(dst.join("a").join("b").join("f.txt")).unwrap(),
            "deep"
        );
        assert_eq!(collected, vec!["a/b/f.txt"]);
    }
}
