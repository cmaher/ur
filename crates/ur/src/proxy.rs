use std::path::Path;

use anyhow::{Context, Result};

/// Read the allowlist file, returning one domain per line.
/// Returns an empty vec if the file does not exist.
pub fn read_allowlist(path: &Path) -> Result<Vec<String>> {
    match std::fs::read_to_string(path) {
        Ok(contents) => Ok(contents
            .lines()
            .map(|l| l.trim().to_string())
            .filter(|l| !l.is_empty())
            .collect()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(e).context("failed to read allowlist.txt"),
    }
}

/// Write the domain list back to the allowlist file, one domain per line.
fn write_allowlist(path: &Path, domains: &[String]) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create directory {}", parent.display()))?;
    }
    let content = domains.join("\n") + "\n";
    std::fs::write(path, content).context("failed to write allowlist.txt")
}

/// Add a domain to the allowlist if not already present. Returns the updated list.
pub fn allow_domain(path: &Path, domain: &str) -> Result<Vec<String>> {
    let mut domains = read_allowlist(path)?;
    if !domains.iter().any(|d| d == domain) {
        domains.push(domain.to_string());
    }
    write_allowlist(path, &domains)?;
    Ok(domains)
}

/// Remove a domain from the allowlist. Returns the updated list.
pub fn block_domain(path: &Path, domain: &str) -> Result<Vec<String>> {
    let mut domains = read_allowlist(path)?;
    domains.retain(|d| d != domain);
    write_allowlist(path, &domains)?;
    Ok(domains)
}

/// Signal the Squid container to reconfigure. Prints a warning on failure
/// but does not error out (the allowlist file was already updated).
pub fn signal_reconfigure(squid_hostname: &str) {
    let status = std::process::Command::new("docker")
        .args(["exec", squid_hostname, "squid", "-k", "reconfigure"])
        .status();
    match status {
        Ok(s) if s.success() => {
            eprintln!("Squid reconfigured.");
        }
        Ok(s) => {
            eprintln!(
                "Warning: squid reconfigure exited with status {}. \
                 The allowlist file was updated but Squid may not have reloaded.",
                s
            );
        }
        Err(e) => {
            eprintln!(
                "Warning: failed to run docker exec: {e}. \
                 The allowlist file was updated but Squid was not reconfigured."
            );
        }
    }
}

/// Print the domain list to stdout.
pub fn print_domains(domains: &[String]) {
    if domains.is_empty() {
        println!("(no domains in allowlist)");
    } else {
        for domain in domains {
            println!("{domain}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn read_missing_file_returns_empty() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("allowlist.txt");
        let domains = read_allowlist(&path).unwrap();
        assert!(domains.is_empty());
    }

    #[test]
    fn read_existing_file() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("allowlist.txt");
        std::fs::write(&path, "api.anthropic.com\nexample.com\n").unwrap();
        let domains = read_allowlist(&path).unwrap();
        assert_eq!(domains, vec!["api.anthropic.com", "example.com"]);
    }

    #[test]
    fn read_ignores_blank_lines() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("allowlist.txt");
        std::fs::write(&path, "api.anthropic.com\n\n  \nexample.com\n").unwrap();
        let domains = read_allowlist(&path).unwrap();
        assert_eq!(domains, vec!["api.anthropic.com", "example.com"]);
    }

    #[test]
    fn allow_adds_new_domain() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("allowlist.txt");
        std::fs::write(&path, "api.anthropic.com\n").unwrap();

        let domains = allow_domain(&path, "example.com").unwrap();
        assert_eq!(domains, vec!["api.anthropic.com", "example.com"]);

        // Verify file was written
        let on_disk = read_allowlist(&path).unwrap();
        assert_eq!(on_disk, domains);
    }

    #[test]
    fn allow_deduplicates() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("allowlist.txt");
        std::fs::write(&path, "api.anthropic.com\n").unwrap();

        let domains = allow_domain(&path, "api.anthropic.com").unwrap();
        assert_eq!(domains, vec!["api.anthropic.com"]);
    }

    #[test]
    fn allow_creates_file_if_missing() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("squid").join("allowlist.txt");

        let domains = allow_domain(&path, "example.com").unwrap();
        assert_eq!(domains, vec!["example.com"]);
        assert!(path.exists());
    }

    #[test]
    fn block_removes_domain() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("allowlist.txt");
        std::fs::write(&path, "api.anthropic.com\nexample.com\n").unwrap();

        let domains = block_domain(&path, "example.com").unwrap();
        assert_eq!(domains, vec!["api.anthropic.com"]);

        let on_disk = read_allowlist(&path).unwrap();
        assert_eq!(on_disk, domains);
    }

    #[test]
    fn block_nonexistent_domain_is_noop() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("allowlist.txt");
        std::fs::write(&path, "api.anthropic.com\n").unwrap();

        let domains = block_domain(&path, "missing.com").unwrap();
        assert_eq!(domains, vec!["api.anthropic.com"]);
    }
}
