use std::path::PathBuf;

use tracing::{info, warn};
use ur_config::AgentType;

/// Copy the baked-in `~/{agent.home_subdir()}/potential-settings.json` to
/// `~/{agent.home_subdir()}/{agent.settings_filename()}` so the agent picks up the
/// hooks/permissions baked into the image. Model selection is NOT injected here — it
/// is passed to the agent's spawn command as a flag at launch time instead, because
/// Claude Code rewrites `~/.claude/settings.json` on startup and silently drops the
/// `model` key (see workerd `run_daemon_only`).
///
/// A no-op for agents with no settings-file concept (`agent.settings_filename()` is `None`).
#[derive(Clone)]
pub struct InitSettingsManager {
    home: PathBuf,
    agent: AgentType,
}

impl InitSettingsManager {
    pub fn from_env() -> Self {
        let home = std::env::var("HOME").unwrap_or_else(|_| ur_config::WORKER_HOME.into());
        InitSettingsManager {
            home: PathBuf::from(home),
            agent: AgentType::from_env(),
        }
    }

    pub async fn run(&self) -> Result<(), std::io::Error> {
        let Some(settings_filename) = self.agent.settings_filename() else {
            info!(
                agent = self.agent.name(),
                "agent has no settings file concept, skipping"
            );
            return Ok(());
        };

        let home_subdir = self.home.join(self.agent.home_subdir());
        let src = home_subdir.join("potential-settings.json");
        let dst = home_subdir.join(settings_filename);

        if !src.exists() {
            warn!(
                path = %src.display(),
                "potential-settings.json not found, skipping settings composition"
            );
            return Ok(());
        }

        tokio::fs::copy(&src, &dst).await?;
        info!(src = %src.display(), dst = %dst.display(), "copied settings file");
        Ok(())
    }
}

#[cfg(test)]
impl InitSettingsManager {
    fn with_home(home: PathBuf) -> Self {
        InitSettingsManager {
            home,
            agent: AgentType::Claude,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn setup_potential_settings(tmp: &TempDir, content: &str) {
        let claude_dir = tmp.path().join(".claude");
        std::fs::create_dir_all(&claude_dir).unwrap();
        std::fs::write(claude_dir.join("potential-settings.json"), content).unwrap();
    }

    const BASE_SETTINGS: &str = r#"{
  "permissions": {
    "defaultMode": "bypassPermissions"
  },
  "skipDangerousModePermissionPrompt": true,
  "hooks": {
    "SessionStart": [
      {
        "matcher": "",
        "hooks": [
          {
            "type": "command",
            "command": "workertools notify-idle"
          }
        ]
      }
    ]
  }
}"#;

    #[tokio::test]
    async fn run_copies_base_file_verbatim() {
        let tmp = TempDir::new().unwrap();
        setup_potential_settings(&tmp, BASE_SETTINGS);

        let mgr = InitSettingsManager::with_home(tmp.path().to_path_buf());
        mgr.run().await.unwrap();

        let written = std::fs::read_to_string(tmp.path().join(".claude/settings.json")).unwrap();
        assert_eq!(
            written, BASE_SETTINGS,
            "settings.json should be a verbatim copy of potential-settings.json"
        );
    }

    #[tokio::test]
    async fn run_missing_base_file_is_noop() {
        let tmp = TempDir::new().unwrap();
        std::fs::create_dir_all(tmp.path().join(".claude")).unwrap();

        let mgr = InitSettingsManager::with_home(tmp.path().to_path_buf());
        mgr.run().await.unwrap();

        assert!(
            !tmp.path().join(".claude/settings.json").exists(),
            "settings.json must not be written when potential-settings.json is absent"
        );
    }
}
