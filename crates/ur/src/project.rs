use std::path::Path;

use anyhow::{Context, Result, bail};
use tracing::{debug, info, warn};
use ur_rpc::proto::core::ReloadProjectsRequest;
use ur_rpc::proto::core::core_service_client::CoreServiceClient;

use crate::connection;
use crate::output::{OutputManager, ProjectAdded, ProjectInfo, ProjectRemoved};

/// Resolve the git remote "origin" URL for a repository directory.
pub(crate) fn git_remote_origin(path: &Path) -> Result<String> {
    let output = std::process::Command::new("git")
        .args(["remote", "get-url", "origin"])
        .current_dir(path)
        .output()
        .context("failed to run git")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!(
            "failed to get git remote origin for '{}': {}",
            path.display(),
            stderr.trim()
        );
    }
    let url = String::from_utf8(output.stdout)
        .context("git remote URL is not valid UTF-8")?
        .trim()
        .to_string();
    if url.is_empty() {
        bail!("git remote origin is empty for '{}'", path.display());
    }
    Ok(url)
}

/// Derive a project key from a git remote URL.
///
/// Takes the last path segment and strips a trailing `.git` suffix.
/// For example, `git@github.com:cmaher/ur.git` becomes `ur`.
pub(crate) fn derive_key_from_repo(repo: &str) -> Result<String> {
    let segment = repo
        .rsplit('/')
        .next()
        .or_else(|| repo.rsplit(':').next())
        .ok_or_else(|| anyhow::anyhow!("cannot derive key from repo URL: {repo}"))?;
    let key = segment.strip_suffix(".git").unwrap_or(segment);
    if key.is_empty() {
        bail!("cannot derive key from repo URL: {repo}");
    }
    Ok(key.to_string())
}

/// List all configured projects.
pub fn list(config: &ur_config::Config, output: &OutputManager) -> Result<()> {
    debug!("listing projects");

    if config.projects.is_empty() {
        output.print_text("No projects configured.");
        return Ok(());
    }

    let pool_base = config.workspace.join("pool");

    // Sort by key for stable output
    let mut projects: Vec<_> = config.projects.values().collect();
    projects.sort_by_key(|p| &p.key);

    let items: Vec<ProjectInfo> = projects
        .iter()
        .map(|proj| {
            // A local project has no pool directory, so don't stat for one.
            let slots_in_use = if proj.is_local() {
                None
            } else {
                Some(count_pool_slots(&pool_base.join(&proj.key)))
            };
            ProjectInfo {
                key: proj.key.clone(),
                repo: proj.repo.clone(),
                name: proj.name.clone(),
                pool_limit: (!proj.is_local()).then_some(proj.pool_limit),
                slots_in_use,
                local: proj.is_local(),
            }
        })
        .collect();

    output.print_items(&items, |items| {
        let mut out = String::new();
        for proj in items {
            // Local projects have no repo and no pool — print what applies to them
            // rather than padding the row with empty or zero columns.
            match (&proj.repo, proj.pool_limit, proj.slots_in_use) {
                (Some(repo), Some(limit), Some(slots)) => out.push_str(&format!(
                    "{key}  repo={repo}  name={name}  pool_limit={limit}  slots={slots}\n",
                    key = proj.key,
                    name = proj.name,
                )),
                _ => out.push_str(&format!(
                    "{key}  local  name={name}\n",
                    key = proj.key,
                    name = proj.name,
                )),
            }
        }
        if out.ends_with('\n') {
            out.pop();
        }
        out
    });

    Ok(())
}

/// Count the number of existing pool slot directories for a project.
fn count_pool_slots(pool_dir: &Path) -> usize {
    match std::fs::read_dir(pool_dir) {
        Ok(entries) => entries
            .filter_map(|e| e.ok())
            .filter(|e| e.path().is_dir())
            .count(),
        Err(_) => 0,
    }
}

/// Derive a project key from a directory path, using its basename.
///
/// Used for local projects, which have no repo URL to derive a key from.
pub(crate) fn derive_key_from_path(path: &Path) -> Result<String> {
    let key = path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| anyhow::anyhow!("cannot derive key from path: {}", path.display()))?;
    if key.is_empty() {
        bail!("cannot derive key from path: {}", path.display());
    }
    Ok(key.to_string())
}

/// Arguments for [`add`], mirroring the `ur project add` flags.
pub struct AddRequest<'a> {
    /// Directory to add. Must be a git repo unless `local` is set.
    pub path: &'a Path,
    /// Container image alias or full reference.
    pub image: &'a str,
    /// Explicit project key. Derived from the repo URL (or the directory name for
    /// a local project) when `None`.
    pub key: Option<&'a str>,
    /// Display-friendly name. Defaults to the key.
    pub name: Option<&'a str>,
    /// Repo pool size. Not valid with `local`.
    pub pool_limit: Option<u32>,
    /// Add a repo-less local project: `local = true`, no `repo`.
    pub local: bool,
}

/// Add a new project to `ur.toml`.
///
/// With `local: false`, resolves the git remote origin from the path and writes a
/// normal pool-backed project. With `local: true`, writes `local = true` and no
/// `repo`: the directory needs no git remote (or may have one we never clone
/// from), the project gets no pool, and its tickets cannot be dispatched. Local
/// projects are used in workspace mode (`ur worker launch -m manual -w <dir>`).
pub fn add(config: &ur_config::Config, req: &AddRequest<'_>, output: &OutputManager) -> Result<()> {
    let AddRequest {
        image,
        name,
        pool_limit,
        local,
        ..
    } = *req;
    let path = std::fs::canonicalize(req.path)
        .with_context(|| format!("failed to resolve path: {}", req.path.display()))?;

    if local && pool_limit.is_some() {
        bail!("--pool-limit is not valid with --local (local projects have no repo pool)");
    }

    ur_config::validate_image_alias(image)
        .map_err(|error| anyhow::anyhow!("invalid container image: {error}"))?;

    // For a local project the directory need not be a git repo at all, so the key
    // comes from the directory basename rather than a remote URL.
    let repo = if local {
        None
    } else {
        Some(git_remote_origin(&path)?)
    };
    let key = match (req.key, &repo) {
        (Some(k), _) => k.to_string(),
        (None, Some(repo)) => derive_key_from_repo(repo)?,
        (None, None) => derive_key_from_path(&path)?,
    };
    info!(key = %key, repo = ?repo, local, path = %path.display(), "adding project");

    if config.projects.contains_key(&key) {
        bail!("project '{key}' already exists — remove it first or choose a different key");
    }

    let toml_path = config.config_dir.join("ur.toml");
    let contents = std::fs::read_to_string(&toml_path)
        .with_context(|| format!("failed to read {}", toml_path.display()))?;

    let mut doc = contents
        .parse::<toml_edit::DocumentMut>()
        .with_context(|| format!("failed to parse {}", toml_path.display()))?;

    // Ensure [projects] table exists
    if !doc.contains_key("projects") {
        doc["projects"] = toml_edit::Item::Table(toml_edit::Table::new());
    }

    let projects = doc["projects"]
        .as_table_mut()
        .ok_or_else(|| anyhow::anyhow!("'projects' in ur.toml is not a table"))?;

    let mut proj_table = toml_edit::Table::new();
    match &repo {
        Some(repo) => proj_table.insert("repo", toml_edit::value(repo)),
        None => proj_table.insert("local", toml_edit::value(true)),
    };
    if let Some(n) = name {
        proj_table.insert("name", toml_edit::value(n));
    }
    if let Some(limit) = pool_limit {
        proj_table.insert("pool_limit", toml_edit::value(i64::from(limit)));
    }

    let mut container_table = toml_edit::Table::new();
    container_table.insert("image", toml_edit::value(image));
    proj_table.insert("container", toml_edit::Item::Table(container_table));

    projects.insert(&key, toml_edit::Item::Table(proj_table));

    std::fs::write(&toml_path, doc.to_string())
        .with_context(|| format!("failed to write {}", toml_path.display()))?;

    info!(key = %key, "project added");
    if output.is_json() {
        output.print_success(&ProjectAdded {
            key: key.clone(),
            repo: repo.clone(),
            local,
        });
    } else {
        match &repo {
            Some(repo) => println!("Added project '{key}' (repo: {repo})"),
            None => println!("Added local project '{key}' (no repo — workspace mode only)"),
        }
    }
    Ok(())
}

/// Remove a project from `ur.toml` and delete its pool directory.
pub fn remove(
    config: &ur_config::Config,
    key: &str,
    force: bool,
    output: &OutputManager,
) -> Result<()> {
    let project = config
        .projects
        .get(key)
        .ok_or_else(|| anyhow::anyhow!("project '{key}' not found in config"))?;

    // `--force` guards against destroying pool clones. A local project has no pool,
    // so removal only edits `ur.toml` and needs no confirmation.
    let is_local = project.is_local();
    if !force && !is_local {
        bail!("--force is required to remove a project (this deletes all pool clones)");
    }

    info!(key = %key, is_local, "removing project");

    // Remove from ur.toml
    let toml_path = config.config_dir.join("ur.toml");
    let contents = std::fs::read_to_string(&toml_path)
        .with_context(|| format!("failed to read {}", toml_path.display()))?;

    let mut doc = contents
        .parse::<toml_edit::DocumentMut>()
        .with_context(|| format!("failed to parse {}", toml_path.display()))?;

    if let Some(projects) = doc.get_mut("projects").and_then(|p| p.as_table_mut()) {
        projects.remove(key);
    }

    std::fs::write(&toml_path, doc.to_string())
        .with_context(|| format!("failed to write {}", toml_path.display()))?;

    info!(key = %key, "project removed from config");

    // Delete pool directory. A local project never had one — skip the stat entirely
    // rather than relying on it happening not to exist.
    let pool_dir = config.workspace.join("pool").join(key);
    if is_local {
        debug!(key, "local project has no pool directory to delete");
    } else if pool_dir.exists() {
        info!(path = %pool_dir.display(), "deleting pool directory");
        std::fs::remove_dir_all(&pool_dir)
            .with_context(|| format!("failed to delete pool directory {}", pool_dir.display()))?;
        if !output.is_json() {
            println!("Deleted pool directory: {}", pool_dir.display());
        }
    } else {
        debug!(path = %pool_dir.display(), "no pool directory to delete");
    }

    if output.is_json() {
        output.print_success(&ProjectRemoved {
            key: key.to_string(),
        });
    } else {
        println!("Removed project '{key}'");
    }
    Ok(())
}

/// Attempt to notify the running server to reload projects from ur.toml.
///
/// This is best-effort: if the server is not running (connection refused) or
/// the RPC fails, we print a message but do not return an error since the
/// config file write already succeeded.
///
/// Reads ur.toml from `config_dir` and ships the bytes inline in the RPC
/// payload so the server parses what we just wrote, not its (possibly stale)
/// bind-mounted view of the file. This eliminates the macOS Docker Desktop
/// bind-mount propagation race that would otherwise produce mid-string TOML
/// parse errors when the server reads its own filesystem view.
pub async fn try_reload_server(port: u16, config_dir: &Path, key: &str, action: &str) {
    let Some(channel) = connection::try_connect(port) else {
        println!("Project {action} in config. Will be available on next server start.");
        return;
    };
    let toml_path = config_dir.join("ur.toml");
    let config_toml = match std::fs::read_to_string(&toml_path) {
        Ok(s) => s,
        Err(e) => {
            warn!(error = %e, path = %toml_path.display(), "failed to read ur.toml for reload");
            println!(
                "Warning: project {action} in config but could not read {} to send: {e}",
                toml_path.display()
            );
            return;
        }
    };
    let mut client = CoreServiceClient::new(channel);
    let req = ReloadProjectsRequest { config_toml };
    match client.reload_projects(req).await {
        Ok(response) => {
            let inner = response.into_inner();
            let key_registered = if action == "removed" {
                inner.removed.iter().any(|k| k == key)
            } else {
                inner.added.iter().any(|k| k == key)
            };
            if key_registered {
                println!("Server reloaded — project {key} now {action}");
            } else {
                println!(
                    "Warning: project {action} in config but server did not register '{key}' — \
                     restart server to apply"
                );
            }
        }
        Err(status) => {
            let code = status.code();
            if code == tonic::Code::Unavailable {
                println!("Project {action} in config. Will be available on next server start.");
            } else {
                warn!(
                    code = ?code,
                    message = status.message(),
                    "ReloadProjects RPC failed"
                );
                println!(
                    "Warning: project {action} in config but server reload failed: {}",
                    status.message()
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn write_config(tmp: &TempDir, toml_content: &str) -> ur_config::Config {
        let full = format!("node_id = \"n\"\n{toml_content}");
        std::fs::write(tmp.path().join("ur.toml"), full).unwrap();
        ur_config::Config::load_from(tmp.path()).unwrap()
    }

    /// Create a temporary git repo with a configured remote origin.
    fn make_git_repo(remote_url: &str) -> TempDir {
        let repo_dir = TempDir::new().unwrap();
        std::process::Command::new("git")
            .args(["init", "--initial-branch=main"])
            .current_dir(repo_dir.path())
            .output()
            .unwrap();
        std::process::Command::new("git")
            .args(["remote", "add", "origin", remote_url])
            .current_dir(repo_dir.path())
            .output()
            .unwrap();
        repo_dir
    }

    fn text_output() -> OutputManager {
        OutputManager::from_args(Some("text"))
    }

    #[test]
    fn derive_key_from_ssh_url() {
        assert_eq!(
            derive_key_from_repo("git@github.com:cmaher/ur.git").unwrap(),
            "ur"
        );
    }

    #[test]
    fn derive_key_from_https_url() {
        assert_eq!(
            derive_key_from_repo("https://github.com/cmaher/ur.git").unwrap(),
            "ur"
        );
    }

    #[test]
    fn derive_key_without_git_suffix() {
        assert_eq!(
            derive_key_from_repo("https://github.com/cmaher/my-repo").unwrap(),
            "my-repo"
        );
    }

    #[test]
    fn git_remote_origin_extracts_url() {
        let repo = make_git_repo("git@github.com:cmaher/ur.git");
        let url = git_remote_origin(repo.path()).unwrap();
        assert_eq!(url, "git@github.com:cmaher/ur.git");
    }

    #[test]
    fn git_remote_origin_fails_for_non_repo() {
        let tmp = TempDir::new().unwrap();
        let err = git_remote_origin(tmp.path()).unwrap_err();
        assert!(err.to_string().contains("failed to get git remote origin"));
    }

    #[test]
    fn add_project_creates_entry() {
        let tmp = TempDir::new().unwrap();
        let config = write_config(&tmp, "");
        let repo = make_git_repo("git@github.com:cmaher/ur.git");
        add(
            &config,
            &AddRequest {
                path: repo.path(),
                image: "ur-worker",
                key: None,
                name: None,
                pool_limit: None,
                local: false,
            },
            &text_output(),
        )
        .unwrap();

        let updated = ur_config::Config::load_from(tmp.path()).unwrap();
        assert!(updated.projects.contains_key("ur"));
        assert_eq!(
            updated.projects["ur"].repo.as_deref(),
            Some("git@github.com:cmaher/ur.git")
        );
        assert_eq!(updated.projects["ur"].container.image, "ur-worker");
    }

    #[test]
    fn add_project_with_explicit_key_and_options() {
        let tmp = TempDir::new().unwrap();
        let config = write_config(&tmp, "");
        let repo = make_git_repo("git@github.com:cmaher/ur.git");
        add(
            &config,
            &AddRequest {
                path: repo.path(),
                image: "registry.example/custom:v1",
                key: Some("mykey"),
                name: Some("My Project"),
                pool_limit: Some(5),
                local: false,
            },
            &text_output(),
        )
        .unwrap();

        let updated = ur_config::Config::load_from(tmp.path()).unwrap();
        let proj = &updated.projects["mykey"];
        assert_eq!(proj.repo.as_deref(), Some("git@github.com:cmaher/ur.git"));
        assert_eq!(proj.name, "My Project");
        assert_eq!(proj.pool_limit, 5);
        assert_eq!(proj.container.image, "registry.example/custom:v1");
    }

    #[test]
    fn add_duplicate_key_fails() {
        let tmp = TempDir::new().unwrap();
        let config = write_config(
            &tmp,
            r#"
[projects.ur]
repo = "git@github.com:cmaher/ur.git"

[projects.ur.container]
image = "ur-worker"
"#,
        );
        let repo = make_git_repo("git@github.com:other/ur.git");
        let err = add(
            &config,
            &AddRequest {
                path: repo.path(),
                image: "ur-worker",
                key: None,
                name: None,
                pool_limit: None,
                local: false,
            },
            &text_output(),
        )
        .unwrap_err();
        assert!(err.to_string().contains("already exists"));
    }

    #[test]
    fn add_project_writes_container_image_toml() {
        let tmp = TempDir::new().unwrap();
        let config = write_config(&tmp, "");
        let repo = make_git_repo("git@github.com:cmaher/myproj.git");
        add(
            &config,
            &AddRequest {
                path: repo.path(),
                image: "ur-worker",
                key: None,
                name: None,
                pool_limit: None,
                local: false,
            },
            &text_output(),
        )
        .unwrap();

        let toml_content = std::fs::read_to_string(tmp.path().join("ur.toml")).unwrap();
        assert!(toml_content.contains("[projects.myproj.container]"));
        assert!(toml_content.contains("image = \"ur-worker\""));
    }

    #[test]
    fn add_project_rejects_invalid_image_without_changing_config() {
        let tmp = TempDir::new().unwrap();
        let config = write_config(&tmp, "");
        let repo = make_git_repo("git@github.com:cmaher/myproj.git");
        let original = std::fs::read_to_string(tmp.path().join("ur.toml")).unwrap();

        let error = add(
            &config,
            &AddRequest {
                path: repo.path(),
                image: "not-a-valid-image-alias",
                key: None,
                name: None,
                pool_limit: None,
                local: false,
            },
            &text_output(),
        )
        .unwrap_err();

        assert!(error.to_string().contains("unknown image alias"));
        assert_eq!(
            std::fs::read_to_string(tmp.path().join("ur.toml")).unwrap(),
            original
        );
    }

    #[test]
    fn validate_image_alias_unknown_errors() {
        let err = ur_config::validate_image_alias("unknown").unwrap_err();
        assert!(err.to_string().contains("unknown image alias"));
    }

    #[test]
    fn validate_image_alias_known_ok() {
        ur_config::validate_image_alias("ur-worker").unwrap();
    }

    #[test]
    fn validate_image_alias_full_reference_ok() {
        ur_config::validate_image_alias("myregistry/myimage:v1").unwrap();
    }

    #[test]
    fn remove_project_deletes_config_and_pool() {
        let tmp = TempDir::new().unwrap();
        let config = write_config(
            &tmp,
            r#"
[projects.ur]
repo = "git@github.com:cmaher/ur.git"

[projects.ur.container]
image = "ur-worker"
"#,
        );

        // Create a fake pool directory
        let pool_dir = config.workspace.join("pool").join("ur");
        std::fs::create_dir_all(&pool_dir).unwrap();
        std::fs::create_dir(pool_dir.join("0")).unwrap();

        remove(&config, "ur", true, &text_output()).unwrap();

        let updated = ur_config::Config::load_from(tmp.path()).unwrap();
        assert!(!updated.projects.contains_key("ur"));
        assert!(!pool_dir.exists());
    }

    #[test]
    fn remove_without_force_fails() {
        let tmp = TempDir::new().unwrap();
        let config = write_config(
            &tmp,
            r#"
[projects.ur]
repo = "git@github.com:cmaher/ur.git"

[projects.ur.container]
image = "ur-worker"
"#,
        );
        let err = remove(&config, "ur", false, &text_output()).unwrap_err();
        assert!(err.to_string().contains("--force"));
    }

    #[test]
    fn remove_nonexistent_project_fails() {
        let tmp = TempDir::new().unwrap();
        let config = write_config(&tmp, "");
        let err = remove(&config, "nope", true, &text_output()).unwrap_err();
        assert!(err.to_string().contains("not found"));
    }

    // ── local projects (--local) ───────────────────────────────────────

    /// A plain directory with no git repo at all — the case `--local` exists for.
    fn make_plain_dir(name: &str) -> TempDir {
        let parent = TempDir::new().unwrap();
        std::fs::create_dir(parent.path().join(name)).unwrap();
        parent
    }

    #[test]
    fn add_local_project_writes_local_flag_and_no_repo() {
        let tmp = TempDir::new().unwrap();
        let config = write_config(&tmp, "");
        let parent = make_plain_dir("myapp");
        add(
            &config,
            &AddRequest {
                path: &parent.path().join("myapp"),
                image: "ur-worker",
                key: None,
                name: None,
                pool_limit: None,
                local: true,
            },
            &text_output(),
        )
        .unwrap();

        let updated = ur_config::Config::load_from(tmp.path()).unwrap();
        let proj = &updated.projects["myapp"];
        assert!(proj.is_local());
        assert_eq!(proj.repo, None);
        assert_eq!(proj.container.image, "ur-worker");

        // The written TOML must carry `local = true`, not an empty `repo`.
        let raw = std::fs::read_to_string(tmp.path().join("ur.toml")).unwrap();
        assert!(raw.contains("local = true"), "unexpected toml: {raw}");
        assert!(!raw.contains("repo ="), "unexpected toml: {raw}");
    }

    #[test]
    fn add_local_project_derives_key_from_directory_name() {
        let tmp = TempDir::new().unwrap();
        let config = write_config(&tmp, "");
        let parent = make_plain_dir("some-tool");
        add(
            &config,
            &AddRequest {
                path: &parent.path().join("some-tool"),
                image: "ur-worker",
                key: None,
                name: None,
                pool_limit: None,
                local: true,
            },
            &text_output(),
        )
        .unwrap();
        let updated = ur_config::Config::load_from(tmp.path()).unwrap();
        assert!(updated.projects.contains_key("some-tool"));
    }

    #[test]
    fn add_local_project_honors_explicit_key_and_name() {
        let tmp = TempDir::new().unwrap();
        let config = write_config(&tmp, "");
        let parent = make_plain_dir("myapp");
        add(
            &config,
            &AddRequest {
                path: &parent.path().join("myapp"),
                image: "ur-worker",
                key: Some("mykey"),
                name: Some("My App"),
                pool_limit: None,
                local: true,
            },
            &text_output(),
        )
        .unwrap();
        let updated = ur_config::Config::load_from(tmp.path()).unwrap();
        let proj = &updated.projects["mykey"];
        assert!(proj.is_local());
        assert_eq!(proj.name, "My App");
    }

    #[test]
    fn add_local_project_rejects_pool_limit() {
        let tmp = TempDir::new().unwrap();
        let config = write_config(&tmp, "");
        let parent = make_plain_dir("myapp");
        let err = add(
            &config,
            &AddRequest {
                path: &parent.path().join("myapp"),
                image: "ur-worker",
                key: None,
                name: None,
                pool_limit: Some(5),
                local: true,
            },
            &text_output(),
        )
        .unwrap_err();
        assert!(err.to_string().contains("--pool-limit"));
    }

    #[test]
    fn add_local_project_ignores_git_remote_when_directory_is_a_repo() {
        // `--local` is an explicit choice: a directory that happens to be a git
        // repo still becomes a local project, with no `repo` recorded.
        let tmp = TempDir::new().unwrap();
        let config = write_config(&tmp, "");
        let repo = make_git_repo("git@github.com:cmaher/ur.git");
        add(
            &config,
            &AddRequest {
                path: repo.path(),
                image: "ur-worker",
                key: Some("mykey"),
                name: None,
                pool_limit: None,
                local: true,
            },
            &text_output(),
        )
        .unwrap();
        let updated = ur_config::Config::load_from(tmp.path()).unwrap();
        assert!(updated.projects["mykey"].is_local());
        assert_eq!(updated.projects["mykey"].repo, None);
    }

    #[test]
    fn remove_local_project_needs_no_force_and_touches_no_pool() {
        let tmp = TempDir::new().unwrap();
        let config = write_config(
            &tmp,
            r#"
[projects.myapp]
local = true

[projects.myapp.container]
image = "ur-worker"
"#,
        );

        // A stray directory at the pool path must survive: a local project has no
        // pool, so removal has no business deleting anything there.
        let pool_dir = config.workspace.join("pool").join("myapp");
        std::fs::create_dir_all(&pool_dir).unwrap();

        remove(&config, "myapp", false, &text_output()).unwrap();

        let updated = ur_config::Config::load_from(tmp.path()).unwrap();
        assert!(!updated.projects.contains_key("myapp"));
        assert!(pool_dir.exists());
    }

    #[test]
    fn list_renders_local_project_without_repo_or_pool_columns() {
        let tmp = TempDir::new().unwrap();
        let config = write_config(
            &tmp,
            r#"
[projects.myapp]
local = true

[projects.myapp.container]
image = "ur-worker"
"#,
        );
        // Should not error and should not claim a repo or pool.
        list(&config, &text_output()).unwrap();
    }

    #[test]
    fn derive_key_from_path_uses_basename() {
        assert_eq!(
            derive_key_from_path(Path::new("/Users/me/projects/myapp")).unwrap(),
            "myapp"
        );
    }

    #[test]
    fn list_empty_projects() {
        let tmp = TempDir::new().unwrap();
        let config = write_config(&tmp, "");
        // Should not error
        list(&config, &text_output()).unwrap();
    }

    #[test]
    fn count_pool_slots_counts_directories() {
        let tmp = TempDir::new().unwrap();
        let pool_dir = tmp.path().join("pool").join("test");
        std::fs::create_dir_all(&pool_dir).unwrap();
        std::fs::create_dir(pool_dir.join("0")).unwrap();
        std::fs::create_dir(pool_dir.join("1")).unwrap();
        // A file should not be counted
        std::fs::write(pool_dir.join("not-a-slot"), "").unwrap();
        assert_eq!(count_pool_slots(&pool_dir), 2);
    }

    #[test]
    fn count_pool_slots_missing_dir() {
        assert_eq!(count_pool_slots(Path::new("/nonexistent")), 0);
    }
}
