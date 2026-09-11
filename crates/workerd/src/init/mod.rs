pub mod instructions;
pub mod settings;
pub mod skills;
pub mod startup_hooks;

pub use instructions::InitInstructionsManager;
pub use settings::InitSettingsManager;
pub use skills::InitSkillsManager;
pub use startup_hooks::StartupHooksManager;

use std::path::{Path, PathBuf};

/// Resolve the worker's home directory from `$HOME`, falling back to the
/// container's known home. Resolved once in `run_init` and injected into each
/// init manager rather than re-read per manager.
pub fn worker_home() -> PathBuf {
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(ur_config::WORKER_HOME))
}

/// Recursively copy `src` into `dst`, creating directories as needed.
pub(crate) async fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<(), std::io::Error> {
    tokio::fs::create_dir_all(dst).await?;

    let mut entries = tokio::fs::read_dir(src).await?;
    while let Some(entry) = entries.next_entry().await? {
        let entry_type = entry.file_type().await?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());

        if entry_type.is_dir() {
            Box::pin(copy_dir_recursive(&src_path, &dst_path)).await?;
        } else {
            tokio::fs::copy(&src_path, &dst_path).await?;
        }
    }

    Ok(())
}

/// Collect sorted `.md` file paths from a directory. Returns empty vec if dir doesn't exist.
pub(crate) async fn collect_md_files(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let Ok(mut entries) = tokio::fs::read_dir(dir).await else {
        return files;
    };
    while let Ok(Some(entry)) = entries.next_entry().await {
        let path = entry.path();
        if path.extension().is_some_and(|ext| ext == "md") {
            files.push(path);
        }
    }
    files.sort();
    files
}
