use std::path::PathBuf;

use tracing::info;

const IN_REPO_GIT_HOOKS: &str = "/workspace/ur-hooks/git";
const HOST_OVERLAY_GIT_HOOKS: &str = "/var/ur/host-hooks/git";
const WORKSPACE_GIT_HOOKS: &str = "/workspace/.git/hooks";

/// Manages copying git hooks from two source directories into the workspace `.git/hooks/`.
///
/// Source resolution order (both are always checked, independently):
/// 1. `/workspace/ur-hooks/git/` (in-repo convention)
/// 2. `/var/ur/host-hooks/git/` (host overlay — wins on identical filenames)
///
/// Missing source directory is a no-op for that side. Files are `chmod 0o755` after copy.
#[derive(Clone)]
pub struct InitGitHooksManager;

impl InitGitHooksManager {
    pub async fn run(&self) -> Result<(), std::io::Error> {
        let target_dir = PathBuf::from(WORKSPACE_GIT_HOOKS);
        copy_git_hooks_from(PathBuf::from(IN_REPO_GIT_HOOKS), &target_dir).await?;
        copy_git_hooks_from(PathBuf::from(HOST_OVERLAY_GIT_HOOKS), &target_dir).await?;
        Ok(())
    }
}

/// Copy all files from `source_dir` into `target_dir`, creating target if needed.
/// Missing `source_dir` is a no-op. Each copied file is set to `0o755`.
async fn copy_git_hooks_from(
    source_dir: PathBuf,
    target_dir: &PathBuf,
) -> Result<(), std::io::Error> {
    if !source_dir.exists() {
        return Ok(());
    }

    info!(source = %source_dir.display(), "initializing git hooks from source");

    tokio::fs::create_dir_all(target_dir).await?;

    let mut entries = tokio::fs::read_dir(&source_dir).await?;
    while let Some(entry) = entries.next_entry().await? {
        let file_type = entry.file_type().await?;
        if !file_type.is_file() {
            continue;
        }

        let src_path = entry.path();
        let dst_path = target_dir.join(entry.file_name());

        tokio::fs::copy(&src_path, &dst_path).await?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = std::fs::Permissions::from_mode(0o755);
            tokio::fs::set_permissions(&dst_path, perms).await?;
        }

        info!(
            file = %entry.file_name().to_string_lossy(),
            src = %src_path.display(),
            dst = %dst_path.display(),
            "copied git hook"
        );
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    /// Build a test manager that copies into a given target_dir, using given source dirs.
    /// We test the internal `copy_git_hooks_from` function directly plus the integration
    /// of two-layer merge via a helper.
    async fn run_merge(
        in_repo: Option<Vec<(&str, &str)>>,
        overlay: Option<Vec<(&str, &str)>>,
    ) -> (TempDir, PathBuf) {
        let tmp = TempDir::new().unwrap();
        let target_dir = tmp.path().join("git-hooks");

        let in_repo_dir = tmp.path().join("in-repo");
        let overlay_dir = tmp.path().join("overlay");

        if let Some(files) = in_repo {
            fs::create_dir_all(&in_repo_dir).unwrap();
            for (name, content) in files {
                fs::write(in_repo_dir.join(name), content).unwrap();
            }
        }

        if let Some(files) = overlay {
            fs::create_dir_all(&overlay_dir).unwrap();
            for (name, content) in files {
                fs::write(overlay_dir.join(name), content).unwrap();
            }
        }

        copy_git_hooks_from(in_repo_dir, &target_dir).await.unwrap();
        copy_git_hooks_from(overlay_dir, &target_dir).await.unwrap();

        (tmp, target_dir)
    }

    #[tokio::test]
    async fn both_sources_overlay_wins_on_conflict() {
        let (_tmp, target) = run_merge(
            Some(vec![("pre-push", "in-repo"), ("commit-msg", "in-repo")]),
            Some(vec![("pre-push", "overlay"), ("pre-commit", "overlay")]),
        )
        .await;

        // overlay wins for pre-push
        assert_eq!(
            fs::read_to_string(target.join("pre-push")).unwrap(),
            "overlay"
        );
        // in-repo only file is preserved
        assert_eq!(
            fs::read_to_string(target.join("commit-msg")).unwrap(),
            "in-repo"
        );
        // overlay only file is present
        assert_eq!(
            fs::read_to_string(target.join("pre-commit")).unwrap(),
            "overlay"
        );
    }

    #[tokio::test]
    async fn only_in_repo_source() {
        let (_tmp, target) = run_merge(
            Some(vec![("pre-push", "in-repo-content")]),
            None, // no overlay dir
        )
        .await;

        assert_eq!(
            fs::read_to_string(target.join("pre-push")).unwrap(),
            "in-repo-content"
        );
    }

    #[tokio::test]
    async fn only_overlay_source() {
        let (_tmp, target) = run_merge(
            None, // no in-repo dir
            Some(vec![("pre-push", "overlay-content")]),
        )
        .await;

        assert_eq!(
            fs::read_to_string(target.join("pre-push")).unwrap(),
            "overlay-content"
        );
    }

    #[tokio::test]
    async fn neither_source_is_noop() {
        let (_tmp, target) = run_merge(None, None).await;

        // target dir should not have been created
        assert!(!target.exists());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn copied_files_are_executable() {
        use std::os::unix::fs::PermissionsExt;

        let (_tmp, target) = run_merge(
            Some(vec![("pre-push", "#!/bin/sh")]),
            Some(vec![("pre-commit", "#!/bin/sh")]),
        )
        .await;

        let mode_push = fs::metadata(target.join("pre-push"))
            .unwrap()
            .permissions()
            .mode();
        let mode_commit = fs::metadata(target.join("pre-commit"))
            .unwrap()
            .permissions()
            .mode();

        assert_eq!(mode_push & 0o777, 0o755);
        assert_eq!(mode_commit & 0o777, 0o755);
    }
}
