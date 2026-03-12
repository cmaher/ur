use std::path::PathBuf;

/// A resolved template path, indicating whether the path is relative to the
/// project root (inside the container) or an absolute host-side path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolvedTemplatePath {
    /// Path relative to the project/repo root, resolved from a `%PROJECT%/...` template.
    /// The inner `PathBuf` is the path *after* the `%PROJECT%/` prefix (e.g., `.git-hooks`).
    ProjectRelative(PathBuf),
    /// Absolute host-side path, resolved from a `%URCONFIG%/...` template or a literal
    /// absolute path. The inner `PathBuf` is fully resolved and ready to use on the host.
    HostPath(PathBuf),
}

/// Recognized template variable prefix for project-relative paths.
const PROJECT_VAR: &str = "%PROJECT%";

/// Recognized template variable prefix for config-directory-relative paths.
const URCONFIG_VAR: &str = "%URCONFIG%";

/// Resolve a template path string into a [`ResolvedTemplatePath`].
///
/// - `%PROJECT%/...` produces [`ResolvedTemplatePath::ProjectRelative`] with the suffix path.
/// - `%URCONFIG%/...` produces [`ResolvedTemplatePath::HostPath`] joined to `config_dir`.
/// - Absolute paths (starting with `/`) produce [`ResolvedTemplatePath::HostPath`] as-is.
/// - Anything else is an error.
///
/// The caller must have already validated the template string with [`validate_template_str`]
/// at config load time, but this function also returns errors for safety.
pub fn resolve_template_path(
    template: &str,
    config_dir: &std::path::Path,
) -> anyhow::Result<ResolvedTemplatePath> {
    if let Some(suffix) = template.strip_prefix(PROJECT_VAR) {
        let suffix = suffix.strip_prefix('/').unwrap_or(suffix);
        Ok(ResolvedTemplatePath::ProjectRelative(PathBuf::from(suffix)))
    } else if let Some(suffix) = template.strip_prefix(URCONFIG_VAR) {
        let suffix = suffix.strip_prefix('/').unwrap_or(suffix);
        Ok(ResolvedTemplatePath::HostPath(config_dir.join(suffix)))
    } else if template.starts_with('/') {
        Ok(ResolvedTemplatePath::HostPath(PathBuf::from(template)))
    } else {
        anyhow::bail!(
            "template path must start with %PROJECT%, %URCONFIG%, or be an absolute path: {template}"
        );
    }
}

/// Validate a template string at config load time.
///
/// Ensures that any `%VAR%` patterns in the string are recognized (`%PROJECT%` or `%URCONFIG%`).
/// Returns an error for unrecognized variables.
pub fn validate_template_str(template: &str) -> anyhow::Result<()> {
    // Find all %VAR% patterns and check they are recognized.
    let mut search_from = 0;
    let bytes = template.as_bytes();
    while search_from < bytes.len() {
        let Some(start) = template[search_from..].find('%') else {
            break;
        };
        let start = search_from + start;
        let after_start = start + 1;
        if after_start >= bytes.len() {
            break;
        }
        let Some(end) = template[after_start..].find('%') else {
            // Lone trailing % — not a variable pattern, ignore.
            break;
        };
        let end = after_start + end;
        let var = &template[start..=end]; // includes both %
        if var != PROJECT_VAR && var != URCONFIG_VAR {
            anyhow::bail!(
                "unrecognized template variable {var} (recognized: %PROJECT%, %URCONFIG%)"
            );
        }
        search_from = end + 1;
    }

    // Also ensure the template is not empty and makes structural sense.
    if template.is_empty() {
        anyhow::bail!("template path must not be empty");
    }

    // If it doesn't start with a recognized prefix or /, that's a validation error too.
    if !template.starts_with(PROJECT_VAR)
        && !template.starts_with(URCONFIG_VAR)
        && !template.starts_with('/')
    {
        anyhow::bail!(
            "template path must start with %PROJECT%, %URCONFIG%, or be an absolute path: {template}"
        );
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn resolve_project_relative_path() {
        let result = resolve_template_path("%PROJECT%/.git-hooks", Path::new("/unused")).unwrap();
        assert_eq!(
            result,
            ResolvedTemplatePath::ProjectRelative(PathBuf::from(".git-hooks"))
        );
    }

    #[test]
    fn resolve_project_relative_nested_path() {
        let result =
            resolve_template_path("%PROJECT%/scripts/hooks", Path::new("/unused")).unwrap();
        assert_eq!(
            result,
            ResolvedTemplatePath::ProjectRelative(PathBuf::from("scripts/hooks"))
        );
    }

    #[test]
    fn resolve_urconfig_path() {
        let config_dir = Path::new("/home/user/.ur");
        let result = resolve_template_path("%URCONFIG%/hooks/myproject", config_dir).unwrap();
        assert_eq!(
            result,
            ResolvedTemplatePath::HostPath(PathBuf::from("/home/user/.ur/hooks/myproject"))
        );
    }

    #[test]
    fn resolve_absolute_path() {
        let result =
            resolve_template_path("/opt/git-hooks/myproject", Path::new("/unused")).unwrap();
        assert_eq!(
            result,
            ResolvedTemplatePath::HostPath(PathBuf::from("/opt/git-hooks/myproject"))
        );
    }

    #[test]
    fn resolve_bare_project_var() {
        // %PROJECT% with no trailing path — resolves to empty relative path
        let result = resolve_template_path("%PROJECT%", Path::new("/unused")).unwrap();
        assert_eq!(
            result,
            ResolvedTemplatePath::ProjectRelative(PathBuf::from(""))
        );
    }

    #[test]
    fn resolve_bare_urconfig_var() {
        let config_dir = Path::new("/home/user/.ur");
        let result = resolve_template_path("%URCONFIG%", config_dir).unwrap();
        assert_eq!(
            result,
            ResolvedTemplatePath::HostPath(PathBuf::from("/home/user/.ur"))
        );
    }

    #[test]
    fn validate_rejects_unrecognized_variable() {
        let err = validate_template_str("%BADVAR%/hooks").unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("unrecognized template variable %BADVAR%"),
            "{msg}"
        );
    }

    #[test]
    fn validate_rejects_unknown_variable_in_middle() {
        let err = validate_template_str("%PROJECT%/foo/%UNKNOWN%/bar").unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("unrecognized template variable %UNKNOWN%"),
            "{msg}"
        );
    }

    #[test]
    fn validate_accepts_project_template() {
        validate_template_str("%PROJECT%/.git-hooks").unwrap();
    }

    #[test]
    fn validate_accepts_urconfig_template() {
        validate_template_str("%URCONFIG%/hooks/myproject").unwrap();
    }

    #[test]
    fn validate_accepts_absolute_path() {
        validate_template_str("/opt/git-hooks").unwrap();
    }

    #[test]
    fn validate_rejects_relative_path_without_variable() {
        let err = validate_template_str("relative/path").unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("must start with"), "{msg}");
    }

    #[test]
    fn validate_rejects_empty_string() {
        let err = validate_template_str("").unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("must not be empty"), "{msg}");
    }

    #[test]
    fn resolve_errors_on_relative_path() {
        let err = resolve_template_path("relative/path", Path::new("/unused")).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("must start with"), "{msg}");
    }
}
