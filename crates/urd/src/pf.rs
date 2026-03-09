use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use tracing::{info, warn};

const PF_ANCHOR: &str = "com.ur.proxy";
const PF_RULES_FILENAME: &str = "pf-proxy.conf";

/// Manages macOS pf (packet filter) firewall rules for the bridge100 interface.
///
/// Installs rules that allow container traffic only to the host gateway IP,
/// blocking all other outbound TCP from the container subnet. This forces
/// containers to use the forward proxy for external access.
///
/// Requires `sudo` for `pfctl` commands. The pf anchor `com.ur.proxy` must
/// be pre-configured in `/etc/pf.conf` (see `scripts/build/install.sh`).
#[derive(Clone)]
pub struct PfManager {
    config_dir: PathBuf,
}

impl PfManager {
    pub fn new(config_dir: PathBuf) -> Self {
        Self { config_dir }
    }

    /// Install pf rules that restrict bridge100 egress to the given host IP.
    ///
    /// 1. Writes rules to `~/.ur/pf-proxy.conf`
    /// 2. Enables pf if not already enabled
    /// 3. Loads rules into the `com.ur.proxy` anchor
    pub fn install(&self, bridge100_ip: &str) -> Result<()> {
        let rules = generate_rules(bridge100_ip);
        let rules_path = self.rules_path();

        std::fs::write(&rules_path, &rules)
            .with_context(|| format!("failed to write pf rules to {}", rules_path.display()))?;
        info!(path = %rules_path.display(), "wrote pf rules");

        // Enable pf (idempotent — returns success even if already enabled)
        enable_pf().context("failed to enable pf")?;

        // Load rules into anchor
        load_anchor(&rules_path).context("failed to load pf anchor rules")?;
        info!(anchor = PF_ANCHOR, "pf rules loaded");

        Ok(())
    }

    /// Flush all rules from the `com.ur.proxy` anchor and remove the rules file.
    pub fn uninstall(&self) -> Result<()> {
        if let Err(e) = flush_anchor() {
            warn!(anchor = PF_ANCHOR, error = %e, "failed to flush pf anchor");
        } else {
            info!(anchor = PF_ANCHOR, "pf anchor flushed");
        }

        let rules_path = self.rules_path();
        if rules_path.exists() {
            std::fs::remove_file(&rules_path).ok();
        }

        Ok(())
    }

    fn rules_path(&self) -> PathBuf {
        self.config_dir.join(PF_RULES_FILENAME)
    }
}

/// Generate pf rules for the bridge100 interface.
///
/// - Allow containers to reach the host gateway IP on any port (gRPC, proxy, dev servers)
/// - Block all other outbound TCP from the container subnet
pub fn generate_rules(bridge100_ip: &str) -> String {
    format!(
        "# Ur proxy pf rules — managed by urd\n\
         # Allow containers to reach host gateway\n\
         pass out quick on bridge100 proto tcp from bridge100:network to {bridge100_ip}\n\
         # Block all other outbound TCP from containers\n\
         block out quick on bridge100 proto tcp from bridge100:network to any\n"
    )
}

/// Enable pf via `sudo pfctl -e`. Idempotent: succeeds even if already enabled.
fn enable_pf() -> Result<()> {
    let output = Command::new("sudo")
        .args(["pfctl", "-e"])
        .output()
        .context("failed to execute sudo pfctl -e")?;

    // pfctl -e exits with code 0 even if already enabled, but prints to stderr.
    // It can also return exit code 1 with "pf already enabled" which is fine.
    let stderr = String::from_utf8_lossy(&output.stderr);
    if !output.status.success() && !stderr.contains("already enabled") {
        bail!("pfctl -e failed: {}", stderr.trim());
    }

    Ok(())
}

/// Load rules from a file into the pf anchor.
fn load_anchor(rules_path: &Path) -> Result<()> {
    let output = Command::new("sudo")
        .args([
            "pfctl",
            "-a",
            PF_ANCHOR,
            "-f",
            &rules_path.display().to_string(),
        ])
        .output()
        .context("failed to execute sudo pfctl -a")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("pfctl load anchor failed: {}", stderr.trim());
    }

    Ok(())
}

/// Flush all rules from the pf anchor.
fn flush_anchor() -> Result<()> {
    let output = Command::new("sudo")
        .args(["pfctl", "-a", PF_ANCHOR, "-F", "all"])
        .output()
        .context("failed to execute sudo pfctl -a flush")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("pfctl flush anchor failed: {}", stderr.trim());
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generate_rules_contains_pass_and_block() {
        let rules = generate_rules("192.168.64.1");
        assert!(rules.contains(
            "pass out quick on bridge100 proto tcp from bridge100:network to 192.168.64.1"
        ));
        assert!(
            rules.contains("block out quick on bridge100 proto tcp from bridge100:network to any")
        );
    }

    #[test]
    fn generate_rules_uses_provided_ip() {
        let rules = generate_rules("10.0.0.1");
        assert!(rules.contains("to 10.0.0.1"));
        assert!(!rules.contains("192.168"));
    }

    #[test]
    fn generate_rules_pass_before_block() {
        let rules = generate_rules("192.168.64.1");
        let pass_pos = rules.find("pass out quick").expect("pass rule missing");
        let block_pos = rules.find("block out quick").expect("block rule missing");
        assert!(
            pass_pos < block_pos,
            "pass rule must come before block rule"
        );
    }

    #[test]
    fn rules_path_uses_config_dir() {
        let mgr = PfManager::new(PathBuf::from("/home/user/.ur"));
        assert_eq!(
            mgr.rules_path(),
            PathBuf::from("/home/user/.ur/pf-proxy.conf")
        );
    }

    #[test]
    fn generate_rules_ends_with_newline() {
        let rules = generate_rules("192.168.64.1");
        assert!(rules.ends_with('\n'));
    }

    #[test]
    fn generate_rules_has_comment_header() {
        let rules = generate_rules("192.168.64.1");
        assert!(rules.starts_with('#'));
        assert!(rules.contains("managed by urd"));
    }
}
