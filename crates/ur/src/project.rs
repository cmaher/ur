use std::path::Path;

use anyhow::{Context, Result, bail};
use tracing::{debug, info};

use crate::output::{OutputManager, ProjectAdded, ProjectInfo, ProjectRemoved};

/// Resolve the git remote "origin" URL for a repository directory.
fn git_remote_origin(path: &Path) -> Result<String> {
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
fn derive_key_from_repo(repo: &str) -> Result<String> {
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
            let pool_dir = pool_base.join(&proj.key);
            let slots_in_use = count_pool_slots(&pool_dir);
            ProjectInfo {
                key: proj.key.clone(),
                repo: proj.repo.clone(),
                name: proj.name.clone(),
                pool_limit: proj.pool_limit,
                slots_in_use,
            }
        })
        .collect();

    output.print_items(&items, |items| {
        let mut out = String::new();
        for proj in items {
            out.push_str(&format!(
                "{key}  repo={repo}  name={name}  pool_limit={limit}  slots={slots}\n",
                key = proj.key,
                repo = proj.repo,
                name = proj.name,
                limit = proj.pool_limit,
                slots = proj.slots_in_use,
            ));
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

/// Add a new project to `ur.toml` by resolving the git remote origin from a directory.
pub fn add(
    config: &ur_config::Config,
    path: &Path,
    image: &str,
    key: Option<&str>,
    name: Option<&str>,
    pool_limit: Option<u32>,
    output: &OutputManager,
) -> Result<()> {
    let path = std::fs::canonicalize(path)
        .with_context(|| format!("failed to resolve path: {}", path.display()))?;
    let repo = git_remote_origin(&path)?;
    let key = match key {
        Some(k) => k.to_string(),
        None => derive_key_from_repo(&repo)?,
    };
    info!(key = %key, repo = %repo, path = %path.display(), "adding project");

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
    proj_table.insert("repo", toml_edit::value(&repo));
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
            repo,
        });
    } else {
        println!("Added project '{key}' (repo: {repo})");
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
    if !force {
        bail!("--force is required to remove a project (this deletes all pool clones)");
    }

    info!(key = %key, "removing project");

    if !config.projects.contains_key(key) {
        bail!("project '{key}' not found in config");
    }

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

    // Delete pool directory
    let pool_dir = config.workspace.join("pool").join(key);
    if pool_dir.exists() {
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

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn write_config(tmp: &TempDir, toml_content: &str) -> ur_config::Config {
        std::fs::write(tmp.path().join("ur.toml"), toml_content).unwrap();
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
            repo.path(),
            "ur-worker",
            None,
            None,
            None,
            &text_output(),
        )
        .unwrap();

        let updated = ur_config::Config::load_from(tmp.path()).unwrap();
        assert!(updated.projects.contains_key("ur"));
        assert_eq!(updated.projects["ur"].repo, "git@github.com:cmaher/ur.git");
        assert_eq!(updated.projects["ur"].container.image, "ur-worker:latest");
    }

    #[test]
    fn add_project_with_explicit_key_and_options() {
        let tmp = TempDir::new().unwrap();
        let config = write_config(&tmp, "");
        let repo = make_git_repo("git@github.com:cmaher/ur.git");
        add(
            &config,
            repo.path(),
            "ur-worker-rust",
            Some("mykey"),
            Some("My Project"),
            Some(5),
            &text_output(),
        )
        .unwrap();

        let updated = ur_config::Config::load_from(tmp.path()).unwrap();
        let proj = &updated.projects["mykey"];
        assert_eq!(proj.repo, "git@github.com:cmaher/ur.git");
        assert_eq!(proj.name, "My Project");
        assert_eq!(proj.pool_limit, 5);
        assert_eq!(proj.container.image, "ur-worker-rust:latest");
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
            repo.path(),
            "ur-worker",
            None,
            None,
            None,
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
            repo.path(),
            "ur-worker",
            None,
            None,
            None,
            &text_output(),
        )
        .unwrap();

        let toml_content = std::fs::read_to_string(tmp.path().join("ur.toml")).unwrap();
        assert!(toml_content.contains("[projects.myproj.container]"));
        assert!(toml_content.contains("image = \"ur-worker\""));
    }

    #[test]
    fn add_project_with_rust_image() {
        let tmp = TempDir::new().unwrap();
        let config = write_config(&tmp, "");
        let repo = make_git_repo("git@github.com:cmaher/myproj.git");
        add(
            &config,
            repo.path(),
            "ur-worker-rust",
            None,
            None,
            None,
            &text_output(),
        )
        .unwrap();

        let updated = ur_config::Config::load_from(tmp.path()).unwrap();
        assert_eq!(
            updated.projects["myproj"].container.image,
            "ur-worker-rust:latest"
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
        ur_config::validate_image_alias("ur-worker-rust").unwrap();
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
