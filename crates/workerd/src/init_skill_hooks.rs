use std::path::PathBuf;

use tracing::info;

const IN_REPO_SKILL_HOOKS: &str = "/workspace/ur-hooks/skills";
const HOST_OVERLAY_SKILL_HOOKS: &str = "/var/ur/host-hooks/skills";
const CLAUDE_SKILL_HOOKS: &str = "/home/worker/.claude/skill-hooks";

/// Manages copying skill hooks from two source directories into `~/.claude/skill-hooks/`.
///
/// Source resolution order (both are always checked, independently):
/// 1. `/workspace/ur-hooks/skills/` (in-repo convention — copied first)
/// 2. `/var/ur/host-hooks/skills/` (host overlay — wins on identical relative paths)
///
/// Missing source directory is a no-op for that side. Copies are recursive.
#[derive(Clone)]
pub struct InitSkillHooksManager;

impl InitSkillHooksManager {
    pub async fn run(&self) -> Result<(), std::io::Error> {
        let target_dir = PathBuf::from(CLAUDE_SKILL_HOOKS);
        copy_skill_hooks_from(&PathBuf::from(IN_REPO_SKILL_HOOKS), &target_dir).await?;
        copy_skill_hooks_from(&PathBuf::from(HOST_OVERLAY_SKILL_HOOKS), &target_dir).await?;
        Ok(())
    }
}

/// Recursively copy all files from `source_dir` into `target_dir`.
/// Missing `source_dir` is a no-op.
async fn copy_skill_hooks_from(
    source_dir: &PathBuf,
    target_dir: &PathBuf,
) -> Result<(), std::io::Error> {
    if !source_dir.exists() {
        return Ok(());
    }

    info!(source = %source_dir.display(), "initializing skill hooks from source");

    copy_dir_recursive(source_dir, target_dir).await
}

/// Recursively copy a directory tree, creating subdirectories as needed.
async fn copy_dir_recursive(src: &PathBuf, dst: &PathBuf) -> Result<(), std::io::Error> {
    tokio::fs::create_dir_all(dst).await?;

    let mut entries = tokio::fs::read_dir(src).await?;
    while let Some(entry) = entries.next_entry().await? {
        let file_type = entry.file_type().await?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());

        if file_type.is_dir() {
            Box::pin(copy_dir_recursive(&src_path, &dst_path)).await?;
        } else if file_type.is_file() {
            tokio::fs::copy(&src_path, &dst_path).await?;
            info!(
                file = %entry.file_name().to_string_lossy(),
                src = %src_path.display(),
                dst = %dst_path.display(),
                "copied skill hook"
            );
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::Path;
    use tempfile::TempDir;

    fn write_test_file(base: &Path, rel_path: &str, content: &str) {
        let p = base.join(rel_path);
        let parent = p.parent().expect("path has parent");
        fs::create_dir_all(parent).unwrap();
        fs::write(p, content).unwrap();
    }

    fn populate_dir(base: &Path, files: Vec<(&str, &str)>) {
        fs::create_dir_all(base).unwrap();
        for (rel_path, content) in files {
            write_test_file(base, rel_path, content);
        }
    }

    /// Run the two-layer merge using real temp directories.
    async fn run_merge(
        in_repo_files: Option<Vec<(&str, &str)>>,
        overlay_files: Option<Vec<(&str, &str)>>,
    ) -> (TempDir, PathBuf) {
        let tmp = TempDir::new().unwrap();
        let target_dir = tmp.path().join("skill-hooks");

        let in_repo_dir = tmp.path().join("in-repo");
        let overlay_dir = tmp.path().join("overlay");

        if let Some(files) = in_repo_files {
            populate_dir(&in_repo_dir, files);
        }

        if let Some(files) = overlay_files {
            populate_dir(&overlay_dir, files);
        }

        copy_skill_hooks_from(&in_repo_dir, &target_dir)
            .await
            .unwrap();
        copy_skill_hooks_from(&overlay_dir, &target_dir)
            .await
            .unwrap();

        (tmp, target_dir)
    }

    #[tokio::test]
    async fn both_sources_overlay_wins_on_conflict() {
        let (_tmp, target) = run_merge(
            Some(vec![
                ("hook-a.sh", "in-repo-a"),
                ("subdir/hook-b.sh", "in-repo-b"),
            ]),
            Some(vec![
                ("hook-a.sh", "overlay-a"),
                ("subdir/hook-c.sh", "overlay-c"),
            ]),
        )
        .await;

        // overlay wins for hook-a.sh
        assert_eq!(
            fs::read_to_string(target.join("hook-a.sh")).unwrap(),
            "overlay-a"
        );
        // in-repo only file preserved
        assert_eq!(
            fs::read_to_string(target.join("subdir/hook-b.sh")).unwrap(),
            "in-repo-b"
        );
        // overlay only file present
        assert_eq!(
            fs::read_to_string(target.join("subdir/hook-c.sh")).unwrap(),
            "overlay-c"
        );
    }

    #[tokio::test]
    async fn only_in_repo_source() {
        let (_tmp, target) =
            run_merge(Some(vec![("hooks/my-hook.sh", "in-repo-content")]), None).await;

        assert_eq!(
            fs::read_to_string(target.join("hooks/my-hook.sh")).unwrap(),
            "in-repo-content"
        );
    }

    #[tokio::test]
    async fn only_overlay_source() {
        let (_tmp, target) =
            run_merge(None, Some(vec![("hooks/my-hook.sh", "overlay-content")])).await;

        assert_eq!(
            fs::read_to_string(target.join("hooks/my-hook.sh")).unwrap(),
            "overlay-content"
        );
    }

    #[tokio::test]
    async fn neither_source_is_noop() {
        let (_tmp, target) = run_merge(None, None).await;

        // target should not have been created
        assert!(!target.exists());
    }

    #[tokio::test]
    async fn recursive_subdirectory_copy() {
        let (_tmp, target) = run_merge(
            Some(vec![("a/b/c/deep.sh", "deep-in-repo")]),
            Some(vec![
                ("a/b/c/deep.sh", "deep-overlay"),
                ("x/y/new.sh", "new-overlay"),
            ]),
        )
        .await;

        // overlay wins on conflict at deep path
        assert_eq!(
            fs::read_to_string(target.join("a/b/c/deep.sh")).unwrap(),
            "deep-overlay"
        );
        // overlay only file in separate subtree
        assert_eq!(
            fs::read_to_string(target.join("x/y/new.sh")).unwrap(),
            "new-overlay"
        );
    }
}
