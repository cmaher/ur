use std::path::PathBuf;

use tracing::info;

const SKILL_HOOKS_DIR_ENV: &str = "UR_SKILL_HOOKS_DIR";
const DEFAULT_SKILL_HOOKS: &str = "/workspace/ur-hooks/skills";
const CLAUDE_SKILL_HOOKS: &str = "/home/worker/.claude/skill-hooks";

/// Manages copying skill hooks from a source directory into `~/.claude/skill-hooks/`.
///
/// Source resolution order:
/// 1. `UR_SKILL_HOOKS_DIR` env var (set by server from `skill_hooks_dir` config)
/// 2. `/workspace/ur-hooks/skills` (convention-based default)
/// 3. No-op if neither exists
#[derive(Clone)]
pub struct InitSkillHooksManager;

impl InitSkillHooksManager {
    pub async fn run(&self) -> Result<(), std::io::Error> {
        let source_dir = match std::env::var(SKILL_HOOKS_DIR_ENV) {
            Ok(val) if !val.trim().is_empty() => PathBuf::from(val),
            _ => PathBuf::from(DEFAULT_SKILL_HOOKS),
        };

        if !source_dir.exists() {
            return Ok(());
        }

        info!(source = %source_dir.display(), "initializing skill hooks");

        let target_dir = PathBuf::from(CLAUDE_SKILL_HOOKS);
        copy_dir_recursive(&source_dir, &target_dir).await
    }
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
