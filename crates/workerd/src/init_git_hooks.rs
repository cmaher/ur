use std::path::PathBuf;

use tracing::info;

const GIT_HOOKS_DIR_ENV: &str = "UR_GIT_HOOKS_DIR";
const DEFAULT_GIT_HOOKS: &str = "/workspace/ur-hooks/git";
const WORKSPACE_GIT_HOOKS: &str = "/workspace/.git/hooks";

/// Manages copying git hooks from a source directory into the workspace .git/hooks.
///
/// Source resolution order:
/// 1. `UR_GIT_HOOKS_DIR` env var (set by server from `git_hooks_dir` config)
/// 2. `/workspace/ur-hooks/git` (convention-based default)
/// 3. No-op if neither exists
#[derive(Clone)]
pub struct InitGitHooksManager;

impl InitGitHooksManager {
    pub async fn run(&self) -> Result<(), std::io::Error> {
        let source_dir = match std::env::var(GIT_HOOKS_DIR_ENV) {
            Ok(val) if !val.trim().is_empty() => PathBuf::from(val),
            _ => PathBuf::from(DEFAULT_GIT_HOOKS),
        };

        info!(source = %source_dir.display(), "initializing git hooks");

        if !source_dir.exists() {
            return Ok(());
        }

        let target_dir = PathBuf::from(WORKSPACE_GIT_HOOKS);
        tokio::fs::create_dir_all(&target_dir).await?;

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
}
