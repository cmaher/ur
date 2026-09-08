use std::path::PathBuf;

use tracing::{info, warn};
use ur_config::AgentType;

use super::copy_dir_recursive;

const SKILLS_ENV: &str = "UR_WORKER_SKILLS";
const POTENTIAL_SKILLS_DIR: &str = ".agent-shared/potential-skills";

/// Manages skill directory initialization from potential-skills based on an env var.
#[derive(Clone)]
pub struct InitSkillsManager {
    home: PathBuf,
    agent: AgentType,
}

impl InitSkillsManager {
    pub fn new(home: PathBuf, agent: AgentType) -> Self {
        InitSkillsManager { home, agent }
    }

    /// Copy the skills named in `UR_WORKER_SKILLS` from `.agent-shared/potential-skills/`
    /// into `~/{agent.home_subdir()}/{agent.skill_subdir()}/`.
    pub async fn run(&self) -> Result<(), std::io::Error> {
        let skills_dir = self
            .home
            .join(self.agent.home_subdir())
            .join(self.agent.skill_subdir());
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
                info!(
                    env = SKILLS_ENV,
                    "env var empty or missing, no skills to copy"
                );
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

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    use tokio::sync::Mutex;

    // Serialize tests that modify env vars
    static ENV_LOCK: Mutex<()> = Mutex::const_new(());

    #[tokio::test]
    async fn run_copies_named_skills() {
        let _lock = ENV_LOCK.lock().await;
        let tmp = TempDir::new().unwrap();
        let potential_dir = tmp.path().join(POTENTIAL_SKILLS_DIR);
        std::fs::create_dir_all(potential_dir.join("my-skill")).unwrap();
        std::fs::write(potential_dir.join("my-skill/SKILL.md"), "# My Skill").unwrap();

        // SAFETY: tests are serialized via ENV_LOCK
        unsafe { std::env::set_var(SKILLS_ENV, "my-skill") };
        let mgr = InitSkillsManager::new(tmp.path().to_path_buf(), AgentType::Claude);
        mgr.run().await.unwrap();
        unsafe { std::env::remove_var(SKILLS_ENV) };

        let dest = tmp.path().join(".claude/skills/my-skill/SKILL.md");
        assert!(dest.exists(), "skill should be copied");
        assert_eq!(std::fs::read_to_string(&dest).unwrap(), "# My Skill");
    }

    #[tokio::test]
    async fn run_missing_skill_warns_but_succeeds() {
        let _lock = ENV_LOCK.lock().await;
        let tmp = TempDir::new().unwrap();

        // SAFETY: tests are serialized via ENV_LOCK
        unsafe { std::env::set_var(SKILLS_ENV, "nonexistent") };
        let mgr = InitSkillsManager::new(tmp.path().to_path_buf(), AgentType::Claude);
        let result = mgr.run().await;
        unsafe { std::env::remove_var(SKILLS_ENV) };

        assert!(result.is_ok(), "missing skill should not cause an error");
        assert!(!tmp.path().join(".claude/skills/nonexistent").exists());
    }

    #[tokio::test]
    async fn run_unset_env_skips_with_empty_skills_dir() {
        let _lock = ENV_LOCK.lock().await;
        let tmp = TempDir::new().unwrap();

        // SAFETY: tests are serialized via ENV_LOCK
        unsafe { std::env::remove_var(SKILLS_ENV) };
        let mgr = InitSkillsManager::new(tmp.path().to_path_buf(), AgentType::Claude);
        let result = mgr.run().await;

        assert!(result.is_ok(), "unset env should not cause an error");
        assert!(
            tmp.path().join(".claude/skills").exists(),
            "skills dir is always (re)created"
        );
    }

    #[tokio::test]
    async fn run_wipes_existing_skills_dir_first() {
        let _lock = ENV_LOCK.lock().await;
        let tmp = TempDir::new().unwrap();
        let skills_dir = tmp.path().join(".claude/skills");
        std::fs::create_dir_all(skills_dir.join("stale-skill")).unwrap();
        std::fs::write(skills_dir.join("stale-skill/SKILL.md"), "old").unwrap();

        // SAFETY: tests are serialized via ENV_LOCK
        unsafe { std::env::remove_var(SKILLS_ENV) };
        let mgr = InitSkillsManager::new(tmp.path().to_path_buf(), AgentType::Claude);
        mgr.run().await.unwrap();

        assert!(
            !skills_dir.join("stale-skill").exists(),
            "stale skill should be wiped"
        );
    }
}
