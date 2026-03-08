use tracing::warn;

/// Reads credentials from the host environment (macOS Keychain, etc.)
/// and provides them for container injection.
#[derive(Clone)]
pub struct CredentialManager;

impl CredentialManager {
    /// Read Claude Code credentials from the macOS Keychain.
    ///
    /// Runs `security find-generic-password -s "Claude Code-credentials" -w`
    /// and returns the JSON blob on success. Returns `None` if the credential
    /// is missing or the command fails (e.g., on non-macOS hosts).
    pub fn read_claude_credentials(&self) -> Option<String> {
        let output = std::process::Command::new("security")
            .args(["find-generic-password", "-s", "Claude Code-credentials", "-w"])
            .output();

        match output {
            Ok(o) if o.status.success() => {
                let raw = String::from_utf8_lossy(&o.stdout).trim().to_string();
                if raw.is_empty() {
                    warn!("Claude credentials keychain entry is empty");
                    None
                } else {
                    Some(raw)
                }
            }
            Ok(_) => {
                warn!("Claude credentials not found in keychain");
                None
            }
            Err(e) => {
                warn!("failed to read Claude credentials from keychain: {e}");
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_claude_credentials_returns_some_or_none() {
        // This test validates the function runs without panicking.
        // On macOS with the credential present it returns Some; otherwise None.
        let mgr = CredentialManager;
        let result = mgr.read_claude_credentials();
        // We can only assert it doesn't panic — the result depends on the host keychain.
        assert!(result.is_some() || result.is_none());
    }
}
