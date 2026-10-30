use tracing::{info, warn};

use crate::paths::WorkerPaths;

/// Copy the baked-in settings template (`WorkerPaths::settings_template`, e.g.
/// `~/.claude/potential-settings.json`) to `WorkerPaths::settings_dest` (e.g.
/// `~/.claude/settings.json`) so the agent picks up the hooks/permissions baked
/// into the image. Model selection is NOT injected here — it is passed to the
/// agent's spawn command as a flag at launch time instead, because Claude Code
/// rewrites `~/.claude/settings.json` on startup and silently drops the `model`
/// key (see workerd `run_daemon_only`).
///
/// A no-op when either path is `None` (agents with no settings-file concept).
#[derive(Clone)]
pub struct InitSettingsManager {
    paths: WorkerPaths,
}

impl InitSettingsManager {
    pub fn new(paths: WorkerPaths) -> Self {
        InitSettingsManager { paths }
    }

    pub async fn run(&self) -> Result<(), std::io::Error> {
        let (Some(src), Some(dst)) = (&self.paths.settings_template, &self.paths.settings_dest)
        else {
            info!("no settings file for this worker, skipping");
            return Ok(());
        };

        if !src.exists() {
            warn!(
                path = %src.display(),
                "settings template not found, skipping settings composition"
            );
            return Ok(());
        }

        tokio::fs::copy(src, dst).await?;
        info!(src = %src.display(), dst = %dst.display(), "copied settings file");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::temp_paths;
    use tempfile::TempDir;
    use ur_config::AgentType;

    fn setup_potential_settings(tmp: &TempDir, content: &str) {
        let claude_dir = tmp.path().join("home/.claude");
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
            "command": "ur-workertools notify-idle"
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

        let mgr = InitSettingsManager::new(temp_paths(&tmp, AgentType::Claude));
        mgr.run().await.unwrap();

        let written =
            std::fs::read_to_string(tmp.path().join("home/.claude/settings.json")).unwrap();
        assert_eq!(
            written, BASE_SETTINGS,
            "settings.json should be a verbatim copy of potential-settings.json"
        );
    }

    #[tokio::test]
    async fn run_missing_base_file_is_noop() {
        let tmp = TempDir::new().unwrap();
        std::fs::create_dir_all(tmp.path().join("home/.claude")).unwrap();

        let mgr = InitSettingsManager::new(temp_paths(&tmp, AgentType::Claude));
        mgr.run().await.unwrap();

        assert!(
            !tmp.path().join("home/.claude/settings.json").exists(),
            "settings.json must not be written when potential-settings.json is absent"
        );
    }

    #[tokio::test]
    async fn agy_settings_stay_under_runtime_root() {
        let tmp = TempDir::new().unwrap();
        let runtime_dir = tmp.path().join("home/.gemini/antigravity-cli");
        std::fs::create_dir_all(&runtime_dir).unwrap();
        std::fs::write(runtime_dir.join("potential-settings.json"), BASE_SETTINGS).unwrap();

        InitSettingsManager::new(temp_paths(&tmp, AgentType::Agy))
            .run()
            .await
            .unwrap();

        assert_eq!(
            std::fs::read_to_string(runtime_dir.join("settings.json")).unwrap(),
            BASE_SETTINGS
        );
        assert!(
            !tmp.path()
                .join("home/.gemini/config/settings.json")
                .exists()
        );
    }

    #[test]
    fn baked_codex_config_enables_vim_mode_by_default() {
        let config = include_str!("../../../../containers/worker-codex/codex-config.toml")
            .parse::<toml::Value>()
            .expect("baked Codex config must be valid TOML");

        assert_eq!(
            config
                .get("tui")
                .and_then(|tui| tui.get("vim_mode_default"))
                .and_then(toml::Value::as_bool),
            Some(true)
        );
    }
}
