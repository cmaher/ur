use std::path::Path;

use anyhow::{Result, bail};

/// Validate an identifier (ticket IDs, worker IDs, project keys, etc.).
///
/// Allows ASCII alphanumeric, hyphens, underscores, and dots. Max 256 chars.
pub fn validate_id(input: &str, field: &str) -> Result<()> {
    if input.is_empty() {
        bail!("{field} must not be empty");
    }
    if input.len() > 256 {
        bail!(
            "{field} must be at most 256 characters (got {})",
            input.len()
        );
    }
    reject_control_chars(input, field)?;
    if !input
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
    {
        bail!(
            "{field} contains invalid characters — only ASCII alphanumeric, hyphens, underscores, and dots are allowed"
        );
    }
    Ok(())
}

/// Reject control characters (0x00-0x1F except \n and \t, plus 0x7F).
pub fn reject_control_chars(input: &str, field: &str) -> Result<()> {
    for (i, c) in input.chars().enumerate() {
        if c == '\n' || c == '\t' {
            continue;
        }
        if c.is_control() {
            bail!(
                "{field} contains control character at position {i} (U+{:04X})",
                c as u32
            );
        }
    }
    Ok(())
}

/// Reject path traversal (`..` components).
pub fn reject_path_traversal(path: &Path, field: &str) -> Result<()> {
    for component in path.components() {
        if let std::path::Component::ParentDir = component {
            bail!("{field} must not contain '..' path components");
        }
    }
    Ok(())
}

/// Validate a domain name: no control chars, reasonable format.
pub fn validate_domain(input: &str) -> Result<()> {
    if input.is_empty() {
        bail!("domain must not be empty");
    }
    if input.len() > 253 {
        bail!("domain must be at most 253 characters");
    }
    reject_control_chars(input, "domain")?;
    if !input
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.' || c == '_')
    {
        bail!("domain contains invalid characters");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn valid_id() {
        assert!(validate_id("ur-abc12", "id").is_ok());
        assert!(validate_id("task_1.2", "id").is_ok());
        assert!(validate_id("A-Z.0-9_test", "id").is_ok());
    }

    #[test]
    fn empty_id_rejected() {
        assert!(validate_id("", "id").is_err());
    }

    #[test]
    fn id_too_long() {
        let long = "a".repeat(257);
        assert!(validate_id(&long, "id").is_err());
    }

    #[test]
    fn id_with_spaces_rejected() {
        assert!(validate_id("has space", "id").is_err());
    }

    #[test]
    fn id_with_control_chars_rejected() {
        assert!(validate_id("bad\x00id", "id").is_err());
    }

    #[test]
    fn control_chars_allow_newline_tab() {
        assert!(reject_control_chars("hello\nworld\tfoo", "field").is_ok());
    }

    #[test]
    fn control_chars_reject_null() {
        assert!(reject_control_chars("hello\x00world", "field").is_err());
    }

    #[test]
    fn control_chars_reject_del() {
        assert!(reject_control_chars("hello\x7fworld", "field").is_err());
    }

    #[test]
    fn path_traversal_rejected() {
        let p = PathBuf::from("../etc/passwd");
        assert!(reject_path_traversal(&p, "path").is_err());
    }

    #[test]
    fn normal_path_accepted() {
        let p = PathBuf::from("/home/user/workspace");
        assert!(reject_path_traversal(&p, "path").is_ok());
    }

    #[test]
    fn valid_domain() {
        assert!(validate_domain("api.anthropic.com").is_ok());
        assert!(validate_domain("example.com").is_ok());
    }

    #[test]
    fn empty_domain_rejected() {
        assert!(validate_domain("").is_err());
    }

    #[test]
    fn domain_with_spaces_rejected() {
        assert!(validate_domain("bad domain.com").is_err());
    }
}
