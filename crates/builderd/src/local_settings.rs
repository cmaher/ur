//! Generated agent customization for local (host-side) workers: the
//! `--settings` file and the drive-map / host-tools sections appended to the
//! composed instruction file.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

/// Hook command every local session runs on start and stop.
const NOTIFY_IDLE_COMMAND: &str = "ur-workertools notify-idle";

/// `permissions.deny` rules mirroring the main hostexec policy blocks, which
/// are not enforced for local workers. Guardrails, not a sandbox.
const DENY_RULES: &[&str] = &[
    "Bash(gh pr merge:*)",
    "Bash(gh pr close:*)",
    "Bash(gh pr review --approve:*)",
    "Bash(gh pr review --request-changes:*)",
    "Bash(git worktree:*)",
];

/// Deny rule dropped when the project sets `allow_no_verify`.
const NO_VERIFY_DENY_RULE: &str = "Bash(git push --no-verify:*)";

/// One link in a worker's drive, as described in the drive map.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DriveLink {
    /// Link path inside the drive.
    pub link: PathBuf,
    /// Host path the link points at.
    pub source: PathBuf,
    /// `:ro` in config; advisory only for local workers.
    pub read_only: bool,
}

/// Renders the files that customize a local worker's agent session.
#[derive(Clone, Debug, Default)]
pub struct LocalSettingsManager;

impl LocalSettingsManager {
    /// The `.ur/settings.json` document layered over the user's own settings
    /// via `claude --settings`.
    pub fn settings_json(&self, allow_no_verify: bool) -> Value {
        let mut deny: Vec<&str> = DENY_RULES.to_vec();
        if !allow_no_verify {
            deny.push(NO_VERIFY_DENY_RULE);
        }
        let idle_hook = json!([{
            "matcher": "",
            "hooks": [{ "type": "command", "command": NOTIFY_IDLE_COMMAND }],
        }]);
        json!({
            "permissions": {
                "defaultMode": "auto",
                "deny": deny,
            },
            "skipAutoPermissionPrompt": true,
            "hooks": {
                "SessionStart": idle_hook,
                "Stop": idle_hook,
            },
        })
    }

    /// Markdown appended to the composed instruction file: the drive map and
    /// the note that host tools run natively.
    pub fn instructions_appendix(&self, drive: &Path, links: &[DriveLink]) -> String {
        let mut out = format!(
            "# Drive\n\nYour drive is `{}`. It links these host paths:\n\n",
            drive.display()
        );
        if links.is_empty() {
            out.push_str("- (none)\n");
        }
        for link in links {
            out.push_str(&format!(
                "- `{}` -> `{}`",
                link.link.display(),
                link.source.display()
            ));
            if link.read_only {
                out.push_str(" (read-only: advisory — do not write)");
            }
            out.push('\n');
        }
        out.push_str(
            "\n# Host Tools\n\n\
             You run directly on the host, not in a container. `git`, `gh` and other \
             tools run natively with the user's credentials; ur's host-exec policy is \
             NOT applied. Follow the same rules you would in a container: never merge, \
             close or approve pull requests, and never bypass git hooks.\n",
        );
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SETTINGS_GOLDEN: &str = r#"{
  "hooks": {
    "SessionStart": [
      {
        "hooks": [
          {
            "command": "ur-workertools notify-idle",
            "type": "command"
          }
        ],
        "matcher": ""
      }
    ],
    "Stop": [
      {
        "hooks": [
          {
            "command": "ur-workertools notify-idle",
            "type": "command"
          }
        ],
        "matcher": ""
      }
    ]
  },
  "permissions": {
    "defaultMode": "auto",
    "deny": [
      "Bash(gh pr merge:*)",
      "Bash(gh pr close:*)",
      "Bash(gh pr review --approve:*)",
      "Bash(gh pr review --request-changes:*)",
      "Bash(git worktree:*)",
      "Bash(git push --no-verify:*)"
    ]
  },
  "skipAutoPermissionPrompt": true
}"#;

    #[test]
    fn settings_golden() {
        let settings = LocalSettingsManager.settings_json(false);
        assert_eq!(
            serde_json::to_string_pretty(&settings).unwrap(),
            SETTINGS_GOLDEN
        );
    }

    #[test]
    fn allow_no_verify_drops_only_the_no_verify_rule() {
        let settings = LocalSettingsManager.settings_json(true);
        let deny: Vec<&str> = settings["permissions"]["deny"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert_eq!(deny, DENY_RULES);
    }

    #[test]
    fn appendix_golden_marks_read_only_links() {
        let drive = Path::new("/d/w1");
        let links = [
            DriveLink {
                link: "/d/w1/ctx".into(),
                source: "/src/ctx".into(),
                read_only: true,
            },
            DriveLink {
                link: "/d/w1/brain".into(),
                source: "/src/brain".into(),
                read_only: false,
            },
        ];
        assert_eq!(
            LocalSettingsManager.instructions_appendix(drive, &links),
            "# Drive\n\nYour drive is `/d/w1`. It links these host paths:\n\n\
             - `/d/w1/ctx` -> `/src/ctx` (read-only: advisory — do not write)\n\
             - `/d/w1/brain` -> `/src/brain`\n\
             \n# Host Tools\n\n\
             You run directly on the host, not in a container. `git`, `gh` and other \
             tools run natively with the user's credentials; ur's host-exec policy is \
             NOT applied. Follow the same rules you would in a container: never merge, \
             close or approve pull requests, and never bypass git hooks.\n"
        );
    }

    #[test]
    fn appendix_without_links() {
        let out = LocalSettingsManager.instructions_appendix(Path::new("/d/w1"), &[]);
        assert!(out.contains("- (none)\n"));
    }
}
