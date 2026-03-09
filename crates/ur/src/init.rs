use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

const COMPOSE_CONTENT: &str = include_str!("../../../containers/docker-compose.yml");
const DEFAULT_ALLOWLIST: &str = "api.anthropic.com\n";

pub struct InitFlags {
    pub force: bool,
    pub force_config: bool,
    pub force_compose: bool,
    pub force_squid: bool,
}

pub fn run(flags: InitFlags) -> Result<()> {
    let config_dir = ur_config::resolve_config_dir()?;
    run_in(config_dir, flags)
}

fn run_in(config_dir: PathBuf, flags: InitFlags) -> Result<()> {
    init_dir(&config_dir)?;

    let workspace_dir = config_dir.join("workspace");
    init_dir(&workspace_dir)?;

    let squid_dir = config_dir.join("squid");
    init_dir(&squid_dir)?;

    let should_force_config = flags.force || flags.force_config;
    let should_force_compose = flags.force || flags.force_compose;
    let should_force_squid = flags.force || flags.force_squid;

    write_file(
        &config_dir.join("ur.toml"),
        "",
        should_force_config,
        "--force or --force-config",
    )?;
    write_file(
        &config_dir.join("docker-compose.yml"),
        COMPOSE_CONTENT,
        should_force_compose,
        "--force or --force-compose",
    )?;
    write_file(
        &squid_dir.join("allowlist.txt"),
        DEFAULT_ALLOWLIST,
        should_force_squid,
        "--force or --force-squid",
    )?;

    Ok(())
}

fn init_dir(path: &Path) -> Result<()> {
    fs::create_dir_all(path)
        .with_context(|| format!("failed to create directory {}", path.display()))?;
    println!("Created {}", path.display());
    Ok(())
}

fn write_file(path: &PathBuf, content: &str, force: bool, force_hint: &str) -> Result<()> {
    if path.exists() && !force {
        println!(
            "Skipped {} (exists, use {} to overwrite)",
            path.display(),
            force_hint
        );
        return Ok(());
    }
    fs::write(path, content)
        .with_context(|| format!("failed to write {}", path.display()))?;
    println!("Created {}", path.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn flags(force: bool, config: bool, compose: bool, squid: bool) -> InitFlags {
        InitFlags {
            force,
            force_config: config,
            force_compose: compose,
            force_squid: squid,
        }
    }

    fn run_with_dir(dir: &Path, f: InitFlags) -> Result<()> {
        run_in(dir.to_path_buf(), f)
    }

    #[test]
    fn creates_all_files_and_dirs() {
        let tmp = TempDir::new().unwrap();
        run_with_dir(tmp.path(), flags(false, false, false, false)).unwrap();

        assert!(tmp.path().join("workspace").is_dir());
        assert!(tmp.path().join("squid").is_dir());
        assert!(tmp.path().join("ur.toml").exists());
        assert!(tmp.path().join("docker-compose.yml").exists());
        assert!(tmp.path().join("squid/allowlist.txt").exists());
    }

    #[test]
    fn ur_toml_is_empty() {
        let tmp = TempDir::new().unwrap();
        run_with_dir(tmp.path(), flags(false, false, false, false)).unwrap();

        let content = fs::read_to_string(tmp.path().join("ur.toml")).unwrap();
        assert!(content.is_empty());
    }

    #[test]
    fn compose_file_contains_embedded_content() {
        let tmp = TempDir::new().unwrap();
        run_with_dir(tmp.path(), flags(false, false, false, false)).unwrap();

        let content = fs::read_to_string(tmp.path().join("docker-compose.yml")).unwrap();
        assert!(content.contains("ur-server"));
    }

    #[test]
    fn allowlist_contains_anthropic() {
        let tmp = TempDir::new().unwrap();
        run_with_dir(tmp.path(), flags(false, false, false, false)).unwrap();

        let content = fs::read_to_string(tmp.path().join("squid/allowlist.txt")).unwrap();
        assert_eq!(content.trim(), "api.anthropic.com");
    }

    #[test]
    fn skips_existing_files_without_force() {
        let tmp = TempDir::new().unwrap();
        run_with_dir(tmp.path(), flags(false, false, false, false)).unwrap();

        // Modify a file to prove it won't be overwritten
        fs::write(tmp.path().join("ur.toml"), "daemon_port = 9999\n").unwrap();
        run_with_dir(tmp.path(), flags(false, false, false, false)).unwrap();

        let content = fs::read_to_string(tmp.path().join("ur.toml")).unwrap();
        assert_eq!(content, "daemon_port = 9999\n");
    }

    #[test]
    fn force_overwrites_all_files() {
        let tmp = TempDir::new().unwrap();
        run_with_dir(tmp.path(), flags(false, false, false, false)).unwrap();

        fs::write(tmp.path().join("ur.toml"), "daemon_port = 9999\n").unwrap();
        run_with_dir(tmp.path(), flags(true, false, false, false)).unwrap();

        let content = fs::read_to_string(tmp.path().join("ur.toml")).unwrap();
        assert!(content.is_empty());
    }

    #[test]
    fn force_config_only_overwrites_toml() {
        let tmp = TempDir::new().unwrap();
        run_with_dir(tmp.path(), flags(false, false, false, false)).unwrap();

        fs::write(tmp.path().join("ur.toml"), "daemon_port = 9999\n").unwrap();
        fs::write(tmp.path().join("docker-compose.yml"), "custom").unwrap();
        run_with_dir(tmp.path(), flags(false, true, false, false)).unwrap();

        let toml_content = fs::read_to_string(tmp.path().join("ur.toml")).unwrap();
        assert!(toml_content.is_empty(), "ur.toml should be overwritten");

        let compose_content = fs::read_to_string(tmp.path().join("docker-compose.yml")).unwrap();
        assert_eq!(compose_content, "custom", "compose should be untouched");
    }

    #[test]
    fn force_compose_only_overwrites_compose() {
        let tmp = TempDir::new().unwrap();
        run_with_dir(tmp.path(), flags(false, false, false, false)).unwrap();

        fs::write(tmp.path().join("ur.toml"), "daemon_port = 9999\n").unwrap();
        fs::write(tmp.path().join("docker-compose.yml"), "custom").unwrap();
        run_with_dir(tmp.path(), flags(false, false, true, false)).unwrap();

        let toml_content = fs::read_to_string(tmp.path().join("ur.toml")).unwrap();
        assert_eq!(toml_content, "daemon_port = 9999\n", "toml should be untouched");

        let compose_content = fs::read_to_string(tmp.path().join("docker-compose.yml")).unwrap();
        assert!(compose_content.contains("ur-server"), "compose should be overwritten");
    }

    #[test]
    fn force_squid_only_overwrites_allowlist() {
        let tmp = TempDir::new().unwrap();
        run_with_dir(tmp.path(), flags(false, false, false, false)).unwrap();

        fs::write(tmp.path().join("squid/allowlist.txt"), "custom.com\n").unwrap();
        fs::write(tmp.path().join("ur.toml"), "daemon_port = 9999\n").unwrap();
        run_with_dir(tmp.path(), flags(false, false, false, true)).unwrap();

        let allowlist = fs::read_to_string(tmp.path().join("squid/allowlist.txt")).unwrap();
        assert_eq!(allowlist.trim(), "api.anthropic.com", "allowlist should be overwritten");

        let toml_content = fs::read_to_string(tmp.path().join("ur.toml")).unwrap();
        assert_eq!(toml_content, "daemon_port = 9999\n", "toml should be untouched");
    }

    #[test]
    fn idempotent_on_directories() {
        let tmp = TempDir::new().unwrap();
        run_with_dir(tmp.path(), flags(false, false, false, false)).unwrap();
        // Running again should not fail even though dirs exist
        run_with_dir(tmp.path(), flags(false, false, false, false)).unwrap();
        assert!(tmp.path().join("workspace").is_dir());
    }

    #[test]
    fn created_config_is_loadable() {
        let tmp = TempDir::new().unwrap();
        run_with_dir(tmp.path(), flags(false, false, false, false)).unwrap();

        let cfg = ur_config::Config::load_from(tmp.path()).unwrap();
        assert_eq!(cfg.config_dir, tmp.path());
        assert_eq!(cfg.daemon_port, ur_config::DEFAULT_DAEMON_PORT);
    }
}
