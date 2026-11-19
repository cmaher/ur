use std::path::{Path, PathBuf};

/// Template variable for workspace paths, used in builderd working_dir and
/// project CLAUDE.md content. Replaced at runtime with the actual workspace path.
pub const WORKSPACE_TEMPLATE: &str = "%WORKSPACE%";

/// Environment variable: the host user's home directory, set on the server
/// container by compose. `%HOME%` expands against it inside the server, whose
/// own `HOME` is the container's.
pub const UR_HOST_HOME_ENV: &str = "UR_HOST_HOME";

/// Replace all `%WORKSPACE%` occurrences in `content` with `workspace`.
///
/// Returns the content unchanged if it contains no `%WORKSPACE%` patterns.
pub fn resolve_workspace_content(content: &str, workspace: &str) -> String {
    content.replace(WORKSPACE_TEMPLATE, workspace)
}

/// A resolved template path, indicating whether the path is relative to the
/// project root (inside the container) or an absolute host-side path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolvedTemplatePath {
    /// Path relative to the project/repo root, resolved from a `%PROJECT%/...` template.
    /// The inner `PathBuf` is the path *after* the `%PROJECT%/` prefix (e.g., `.git-hooks`).
    ProjectRelative(PathBuf),
    /// Absolute host-side path, resolved from a `%URCONFIG%/...` or `%HOME%/...` template
    /// or a literal absolute path. The inner `PathBuf` is fully resolved and ready to use
    /// on the host.
    HostPath(PathBuf),
}

/// Recognized template variable prefix for project-relative paths.
const PROJECT_VAR: &str = "%PROJECT%";

/// Recognized template variable prefix for config-directory-relative paths.
const URCONFIG_VAR: &str = "%URCONFIG%";

/// Recognized template variable prefix for paths relative to the host user's home.
const HOME_VAR: &str = "%HOME%";

/// The host home directory `%HOME%` expands to, and the env var it came from.
///
/// Host processes (CLI, builderd) read `HOME`. The server container — detected by
/// `UR_HOST_CONFIG` being set, the same signal config loading uses elsewhere —
/// reads [`UR_HOST_HOME_ENV`], because its own `HOME` is not the host's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TemplateHome {
    /// Env var the home directory is read from (named in errors).
    pub source_env: &'static str,
    /// Its value; `None` when unset or empty.
    pub dir: Option<PathBuf>,
}

impl TemplateHome {
    /// Read the home directory for this process from the environment.
    pub fn from_env() -> Self {
        let source_env = if std::env::var_os(crate::UR_HOST_CONFIG_ENV).is_some() {
            UR_HOST_HOME_ENV
        } else {
            "HOME"
        };
        let dir = std::env::var_os(source_env)
            .filter(|value| !value.is_empty())
            .map(PathBuf::from);
        TemplateHome { source_env, dir }
    }

    /// The home directory, or an error naming the unset env var.
    fn require(&self) -> anyhow::Result<&Path> {
        self.dir.as_deref().ok_or_else(|| {
            anyhow::anyhow!(
                "%HOME% cannot be expanded: {} is not set in this process's environment",
                self.source_env
            )
        })
    }
}

/// Resolve a template path string into a [`ResolvedTemplatePath`], expanding
/// `%HOME%` against [`TemplateHome::from_env`].
///
/// - `%PROJECT%/...` produces [`ResolvedTemplatePath::ProjectRelative`] with the suffix path.
/// - `%URCONFIG%/...` produces [`ResolvedTemplatePath::HostPath`] joined to `config_dir`.
/// - `%HOME%/...` produces [`ResolvedTemplatePath::HostPath`] joined to the host home.
/// - Absolute paths (starting with `/`) produce [`ResolvedTemplatePath::HostPath`] as-is.
/// - Anything else is an error.
///
/// The caller must have already validated the template string with [`validate_template_str`]
/// at config load time, but this function also returns errors for safety.
pub fn resolve_template_path(
    template: &str,
    config_dir: &Path,
) -> anyhow::Result<ResolvedTemplatePath> {
    resolve_template_path_with_home(template, config_dir, &TemplateHome::from_env())
}

/// [`resolve_template_path`] with an explicit `%HOME%` source.
pub fn resolve_template_path_with_home(
    template: &str,
    config_dir: &Path,
    home: &TemplateHome,
) -> anyhow::Result<ResolvedTemplatePath> {
    if let Some(suffix) = template.strip_prefix(PROJECT_VAR) {
        Ok(ResolvedTemplatePath::ProjectRelative(PathBuf::from(
            strip_slash(suffix),
        )))
    } else if let Some(suffix) = template.strip_prefix(URCONFIG_VAR) {
        Ok(ResolvedTemplatePath::HostPath(
            config_dir.join(strip_slash(suffix)),
        ))
    } else if let Some(suffix) = template.strip_prefix(HOME_VAR) {
        Ok(ResolvedTemplatePath::HostPath(
            home.require()?.join(strip_slash(suffix)),
        ))
    } else if template.starts_with('/') {
        Ok(ResolvedTemplatePath::HostPath(PathBuf::from(template)))
    } else {
        anyhow::bail!(
            "template path must start with %PROJECT%, %URCONFIG%, %HOME%, or be an absolute path: {template}"
        );
    }
}

fn strip_slash(suffix: &str) -> &str {
    suffix.strip_prefix('/').unwrap_or(suffix)
}

/// Validate a template string at config load time, checking that `%HOME%` can be
/// expanded via [`TemplateHome::from_env`].
///
/// Ensures that any `%VAR%` patterns in the string are recognized (`%PROJECT%`,
/// `%URCONFIG%` or `%HOME%`), and that a `%HOME%` template can actually be expanded
/// in this process. Callers prefix errors with the field name.
pub fn validate_template_str(template: &str) -> anyhow::Result<()> {
    validate_template_str_with_home(template, &TemplateHome::from_env())
}

/// [`validate_template_str`] with an explicit `%HOME%` source.
pub fn validate_template_str_with_home(template: &str, home: &TemplateHome) -> anyhow::Result<()> {
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
        if var != PROJECT_VAR && var != URCONFIG_VAR && var != HOME_VAR {
            anyhow::bail!(
                "unrecognized template variable {var} (recognized: %PROJECT%, %URCONFIG%, %HOME%)"
            );
        }
        search_from = end + 1;
    }

    // Also ensure the template is not empty and makes structural sense.
    if template.is_empty() {
        anyhow::bail!("template path must not be empty");
    }

    if template.starts_with(HOME_VAR) {
        home.require()?;
    } else if !template.starts_with(PROJECT_VAR)
        && !template.starts_with(URCONFIG_VAR)
        && !template.starts_with('/')
    {
        anyhow::bail!(
            "template path must start with %PROJECT%, %URCONFIG%, %HOME%, or be an absolute path: {template}"
        );
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn home(dir: Option<&str>) -> TemplateHome {
        TemplateHome {
            source_env: UR_HOST_HOME_ENV,
            dir: dir.map(PathBuf::from),
        }
    }

    #[test]
    fn resolve_home_path_joins_home_dir() {
        let result = resolve_template_path_with_home(
            "%HOME%/notes/x",
            Path::new("/unused"),
            &home(Some("/Users/me")),
        )
        .unwrap();
        assert_eq!(
            result,
            ResolvedTemplatePath::HostPath(PathBuf::from("/Users/me/notes/x"))
        );
    }

    #[test]
    fn resolve_bare_home_var() {
        let result =
            resolve_template_path_with_home("%HOME%", Path::new("/unused"), &home(Some("/h")))
                .unwrap();
        assert_eq!(result, ResolvedTemplatePath::HostPath(PathBuf::from("/h")));
    }

    #[test]
    fn resolve_home_errors_naming_env_when_unset() {
        let err = resolve_template_path_with_home("%HOME%/x", Path::new("/unused"), &home(None))
            .unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("%HOME%") && msg.contains(UR_HOST_HOME_ENV),
            "{msg}"
        );
    }

    #[test]
    fn resolve_without_home_var_ignores_missing_home() {
        let result =
            resolve_template_path_with_home("%URCONFIG%/x", Path::new("/cfg"), &home(None))
                .unwrap();
        assert_eq!(
            result,
            ResolvedTemplatePath::HostPath(PathBuf::from("/cfg/x"))
        );
    }

    #[test]
    fn validate_accepts_home_template_when_home_known() {
        validate_template_str_with_home("%HOME%/brains/x", &home(Some("/h"))).unwrap();
    }

    #[test]
    fn validate_rejects_home_template_when_home_unset() {
        let err = validate_template_str_with_home("%HOME%/brains/x", &home(None)).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains(UR_HOST_HOME_ENV), "{msg}");
    }

    #[test]
    fn validate_non_home_template_does_not_need_home() {
        validate_template_str_with_home("%URCONFIG%/x", &home(None)).unwrap();
        validate_template_str_with_home("/abs", &home(None)).unwrap();
    }

    #[test]
    fn host_process_reads_home_from_home_env() {
        // Tests run on the host, not in the server container.
        if std::env::var_os(crate::UR_HOST_CONFIG_ENV).is_none() {
            let template_home = TemplateHome::from_env();
            assert_eq!(template_home.source_env, "HOME");
            assert_eq!(
                template_home.dir,
                std::env::var_os("HOME")
                    .filter(|v| !v.is_empty())
                    .map(PathBuf::from)
            );
        }
    }

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

    #[test]
    fn resolve_workspace_content_replaces_single() {
        let result = resolve_workspace_content("%WORKSPACE%/pool/ur/0", "/home/builder/ws");
        assert_eq!(result, "/home/builder/ws/pool/ur/0");
    }

    #[test]
    fn resolve_workspace_content_no_match_passthrough() {
        let result = resolve_workspace_content("/absolute/path", "/home/builder/ws");
        assert_eq!(result, "/absolute/path");
    }

    #[test]
    fn resolve_workspace_content_multiple_occurrences() {
        let result = resolve_workspace_content("src=%WORKSPACE%/a dest=%WORKSPACE%/b", "/ws");
        assert_eq!(result, "src=/ws/a dest=/ws/b");
    }

    #[test]
    fn resolve_workspace_content_empty_input() {
        let result = resolve_workspace_content("", "/ws");
        assert_eq!(result, "");
    }
}
