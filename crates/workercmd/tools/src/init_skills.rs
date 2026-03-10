use std::path::PathBuf;

use tracing::{info, warn};

const SKILLS_ENV: &str = "UR_WORKER_SKILLS";
const POTENTIAL_SKILLS_DIR: &str = ".claude/potential-skills";
const SKILLS_DIR: &str = ".claude/skills";

/// Manages skill directory initialization from potential-skills based on an env var.
#[derive(Clone)]
pub struct InitSkillsManager {
    home: PathBuf,
}

impl InitSkillsManager {
    pub fn from_env() -> Self {
        let home = std::env::var("HOME").unwrap_or_else(|_| ur_config::WORKER_HOME.into());
        InitSkillsManager {
            home: PathBuf::from(home),
        }
    }

    pub async fn run(&self) -> i32 {
        if let Err(e) = self.init_skills().await {
            eprintln!("init-skills failed: {e}");
            return 1;
        }
        0
    }

    async fn init_skills(&self) -> Result<(), std::io::Error> {
        let skills_dir = self.home.join(SKILLS_DIR);
        let potential_dir = self.home.join(POTENTIAL_SKILLS_DIR);

        // Always wipe and recreate skills dir
        if skills_dir.exists() {
            tokio::fs::remove_dir_all(&skills_dir).await?;
            info!(path = %skills_dir.display(), "removed existing skills directory");
        }
        tokio::fs::create_dir_all(&skills_dir).await?;
        info!(path = %skills_dir.display(), "created skills directory");

        let skill_names = match std::env::var(SKILLS_ENV) {
            Ok(val) if !val.trim().is_empty() => val,
            _ => {
                info!(env = SKILLS_ENV, "env var empty or missing, no skills to copy");
                return Ok(());
            }
        };

        let names: Vec<&str> = skill_names.split(',').map(|s| s.trim()).collect();
        info!(count = names.len(), skills = %skill_names, "processing skill list");

        for name in names {
            if name.is_empty() {
                continue;
            }
            let src = potential_dir.join(name);
            let dst = skills_dir.join(name);

            if !src.exists() {
                warn!(skill = name, path = %src.display(), "skill not found in potential-skills");
                continue;
            }

            copy_dir_recursive(&src, &dst).await?;
            info!(skill = name, src = %src.display(), dst = %dst.display(), "copied skill");
        }

        Ok(())
    }
}

async fn copy_dir_recursive(src: &PathBuf, dst: &PathBuf) -> Result<(), std::io::Error> {
    tokio::fs::create_dir_all(dst).await?;

    let mut entries = tokio::fs::read_dir(src).await?;
    while let Some(entry) = entries.next_entry().await? {
        let entry_type = entry.file_type().await?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());

        if entry_type.is_dir() {
            Box::pin(copy_dir_recursive(
                &src_path.to_path_buf(),
                &dst_path.to_path_buf(),
            ))
            .await?;
        } else {
            tokio::fs::copy(&src_path, &dst_path).await?;
        }
    }

    Ok(())
}
