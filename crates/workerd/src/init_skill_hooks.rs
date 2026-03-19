use std::path::PathBuf;

use tracing::{info, warn};

const SKILL_HOOKS_DIR_ENV: &str = "UR_SKILL_HOOKS_DIR";
const CLAUDE_SKILL_HOOKS: &str = "/home/worker/.claude/skill-hooks";

/// Manages copying skill hooks from a source directory into `~/.claude/skill-hooks/`.
#[derive(Clone)]
pub struct InitSkillHooksManager;

impl InitSkillHooksManager {
    pub async fn run(&self) -> Result<(), std::io::Error> {
        let source_dir = match std::env::var(SKILL_HOOKS_DIR_ENV) {
            Ok(val) if !val.trim().is_empty() => PathBuf::from(val),
            _ => {
                return Ok(());
            }
        };

        info!(source = %source_dir.display(), "initializing skill hooks");

        if !source_dir.exists() {
            warn!(
                path = %source_dir.display(),
                "skill hooks source directory does not exist, skipping"
            );
            return Ok(());
        }

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
