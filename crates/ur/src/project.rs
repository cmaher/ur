use std::path::Path;

use anyhow::{Context, Result, bail};
use tracing::{debug, info};

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
pub fn list(config: &ur_config::Config) -> Result<()> {
    debug!("listing projects");

    if config.projects.is_empty() {
        println!("No projects configured.");
        return Ok(());
    }

    let pool_base = config.workspace.join("pool");

    // Sort by key for stable output
    let mut projects: Vec<_> = config.projects.values().collect();
    projects.sort_by_key(|p| &p.key);

    for proj in projects {
        let pool_dir = pool_base.join(&proj.key);
        let slots_in_use = count_pool_slots(&pool_dir);
        println!(
            "{key}  repo={repo}  name={name}  pool_limit={limit}  slots={slots}",
            key = proj.key,
            repo = proj.repo,
            name = proj.name,
            limit = proj.pool_limit,
            slots = slots_in_use,
        );
    }

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

/// Add a new project to `ur.toml`.
pub fn add(
    config: &ur_config::Config,
    repo: &str,
    key: Option<&str>,
    name: Option<&str>,
    pool_limit: Option<u32>,
) -> Result<()> {
    let key = match key {
        Some(k) => k.to_string(),
        None => derive_key_from_repo(repo)?,
    };
    info!(key = %key, repo = %repo, "adding project");

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
    proj_table.insert("repo", toml_edit::value(repo));
    if let Some(n) = name {
        proj_table.insert("name", toml_edit::value(n));
    }
    if let Some(limit) = pool_limit {
        proj_table.insert("pool_limit", toml_edit::value(i64::from(limit)));
    }

    projects.insert(&key, toml_edit::Item::Table(proj_table));

    std::fs::write(&toml_path, doc.to_string())
        .with_context(|| format!("failed to write {}", toml_path.display()))?;

    info!(key = %key, "project added");
    println!("Added project '{key}' (repo: {repo})");
    Ok(())
}

/// Remove a project from `ur.toml` and delete its pool directory.
pub fn remove(config: &ur_config::Config, key: &str, force: bool) -> Result<()> {
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
        println!("Deleted pool directory: {}", pool_dir.display());
    } else {
        debug!(path = %pool_dir.display(), "no pool directory to delete");
    }

    println!("Removed project '{key}'");
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
    fn add_project_creates_entry() {
        let tmp = TempDir::new().unwrap();
        let config = write_config(&tmp, "");
        add(&config, "git@github.com:cmaher/ur.git", None, None, None).unwrap();

        let updated = ur_config::Config::load_from(tmp.path()).unwrap();
        assert!(updated.projects.contains_key("ur"));
        assert_eq!(updated.projects["ur"].repo, "git@github.com:cmaher/ur.git");
    }

    #[test]
    fn add_project_with_explicit_key_and_options() {
        let tmp = TempDir::new().unwrap();
        let config = write_config(&tmp, "");
        add(
            &config,
            "git@github.com:cmaher/ur.git",
            Some("mykey"),
            Some("My Project"),
            Some(5),
        )
        .unwrap();

        let updated = ur_config::Config::load_from(tmp.path()).unwrap();
        let proj = &updated.projects["mykey"];
        assert_eq!(proj.repo, "git@github.com:cmaher/ur.git");
        assert_eq!(proj.name, "My Project");
        assert_eq!(proj.pool_limit, 5);
    }

    #[test]
    fn add_duplicate_key_fails() {
        let tmp = TempDir::new().unwrap();
        let config = write_config(
            &tmp,
            r#"
[projects.ur]
repo = "git@github.com:cmaher/ur.git"
"#,
        );
        let err = add(&config, "git@github.com:other/ur.git", None, None, None).unwrap_err();
        assert!(err.to_string().contains("already exists"));
    }

    #[test]
    fn remove_project_deletes_config_and_pool() {
        let tmp = TempDir::new().unwrap();
        let config = write_config(
            &tmp,
            r#"
[projects.ur]
repo = "git@github.com:cmaher/ur.git"
"#,
        );

        // Create a fake pool directory
        let pool_dir = config.workspace.join("pool").join("ur");
        std::fs::create_dir_all(&pool_dir).unwrap();
        std::fs::create_dir(pool_dir.join("0")).unwrap();

        remove(&config, "ur", true).unwrap();

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
"#,
        );
        let err = remove(&config, "ur", false).unwrap_err();
        assert!(err.to_string().contains("--force"));
    }

    #[test]
    fn remove_nonexistent_project_fails() {
        let tmp = TempDir::new().unwrap();
        let config = write_config(&tmp, "");
        let err = remove(&config, "nope", true).unwrap_err();
        assert!(err.to_string().contains("not found"));
    }

    #[test]
    fn list_empty_projects() {
        let tmp = TempDir::new().unwrap();
        let config = write_config(&tmp, "");
        // Should not error
        list(&config).unwrap();
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
