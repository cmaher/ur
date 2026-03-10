use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

const DEFAULT_ALLOWLIST: &str = "\
api.anthropic.com
platform.claude.com
raw.githubusercontent.com
mcp-proxy.anthropic.com
storage.googleapis.com
index.crates.io
static.crates.io
static.rust-lang.org
";

pub struct InitFlags {
    pub force: bool,
    pub force_config: bool,
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

    let claude_dir = config_dir.join(ur_config::CLAUDE_DIR);
    init_dir(&claude_dir)?;

    let hostexec_dir = config_dir.join(ur_config::HOSTEXEC_DIR);
    init_dir(&hostexec_dir)?;

    let should_force_config = flags.force || flags.force_config;
    let should_force_squid = flags.force || flags.force_squid;

    write_file(
        &config_dir.join("ur.toml"),
        "",
        should_force_config,
        "--force or --force-config",
    )?;
    write_file(
        &squid_dir.join("allowlist.txt"),
        DEFAULT_ALLOWLIST,
        should_force_squid,
        "--force or --force-squid",
    )?;

    // Credentials file must exist on the host for Docker file mounts to work
    // (otherwise Docker creates a directory at the mount path).
    write_file(
        &claude_dir.join(ur_config::CLAUDE_CREDENTIALS_FILENAME),
        "",
        false,
        "--force",
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
    fs::write(path, content).with_context(|| format!("failed to write {}", path.display()))?;
    println!("Created {}", path.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn flags(force: bool, config: bool, squid: bool) -> InitFlags {
        InitFlags {
            force,
            force_config: config,
            force_squid: squid,
        }
    }

    fn run_with_dir(dir: &Path, f: InitFlags) -> Result<()> {
        run_in(dir.to_path_buf(), f)
    }

    #[test]
    fn creates_all_files_and_dirs() {
        let tmp = TempDir::new().unwrap();
        run_with_dir(tmp.path(), flags(false, false, false)).unwrap();

        assert!(tmp.path().join("workspace").is_dir());
        assert!(tmp.path().join("squid").is_dir());
        assert!(tmp.path().join("hostexec").is_dir());
        assert!(tmp.path().join("ur.toml").exists());
        assert!(tmp.path().join("squid/allowlist.txt").exists());
    }

    #[test]
    fn ur_toml_is_empty() {
        let tmp = TempDir::new().unwrap();
        run_with_dir(tmp.path(), flags(false, false, false)).unwrap();

        let content = fs::read_to_string(tmp.path().join("ur.toml")).unwrap();
        assert!(content.is_empty());
    }

    #[test]
    fn allowlist_contains_anthropic() {
        let tmp = TempDir::new().unwrap();
        run_with_dir(tmp.path(), flags(false, false, false)).unwrap();

        let content = fs::read_to_string(tmp.path().join("squid/allowlist.txt")).unwrap();
        assert!(content.contains("api.anthropic.com"));
        assert!(content.contains("platform.claude.com"));
        assert!(content.contains("raw.githubusercontent.com"));
        assert!(content.contains("mcp-proxy.anthropic.com"));
        assert!(content.contains("storage.googleapis.com"));
        assert!(content.contains("index.crates.io"));
        assert!(content.contains("static.crates.io"));
        assert!(content.contains("static.rust-lang.org"));
    }

    #[test]
    fn skips_existing_files_without_force() {
        let tmp = TempDir::new().unwrap();
        run_with_dir(tmp.path(), flags(false, false, false)).unwrap();

        // Modify a file to prove it won't be overwritten
        fs::write(tmp.path().join("ur.toml"), "daemon_port = 9999\n").unwrap();
        run_with_dir(tmp.path(), flags(false, false, false)).unwrap();

        let content = fs::read_to_string(tmp.path().join("ur.toml")).unwrap();
        assert_eq!(content, "daemon_port = 9999\n");
    }

    #[test]
    fn force_overwrites_all_files() {
        let tmp = TempDir::new().unwrap();
        run_with_dir(tmp.path(), flags(false, false, false)).unwrap();

        fs::write(tmp.path().join("ur.toml"), "daemon_port = 9999\n").unwrap();
        run_with_dir(tmp.path(), flags(true, false, false)).unwrap();

        let content = fs::read_to_string(tmp.path().join("ur.toml")).unwrap();
        assert!(content.is_empty());
    }

    #[test]
    fn force_config_only_overwrites_toml() {
        let tmp = TempDir::new().unwrap();
        run_with_dir(tmp.path(), flags(false, false, false)).unwrap();

        fs::write(tmp.path().join("ur.toml"), "daemon_port = 9999\n").unwrap();
        run_with_dir(tmp.path(), flags(false, true, false)).unwrap();

        let toml_content = fs::read_to_string(tmp.path().join("ur.toml")).unwrap();
        assert!(toml_content.is_empty(), "ur.toml should be overwritten");
    }

    #[test]
    fn force_squid_overwrites_squid_dir() {
        let tmp = TempDir::new().unwrap();
        run_with_dir(tmp.path(), flags(false, false, false)).unwrap();

        fs::write(tmp.path().join("squid/allowlist.txt"), "custom.com\n").unwrap();
        fs::write(tmp.path().join("ur.toml"), "daemon_port = 9999\n").unwrap();
        run_with_dir(tmp.path(), flags(false, false, true)).unwrap();

        let allowlist = fs::read_to_string(tmp.path().join("squid/allowlist.txt")).unwrap();
        assert!(
            allowlist.contains("api.anthropic.com"),
            "allowlist should be overwritten"
        );
        assert!(
            allowlist.contains("platform.claude.com"),
            "allowlist should be overwritten"
        );

        let toml_content = fs::read_to_string(tmp.path().join("ur.toml")).unwrap();
        assert_eq!(
            toml_content, "daemon_port = 9999\n",
            "toml should be untouched"
        );
    }

    #[test]
    fn idempotent_on_directories() {
        let tmp = TempDir::new().unwrap();
        run_with_dir(tmp.path(), flags(false, false, false)).unwrap();
        // Running again should not fail even though dirs exist
        run_with_dir(tmp.path(), flags(false, false, false)).unwrap();
        assert!(tmp.path().join("workspace").is_dir());
    }

    #[test]
    fn created_config_is_loadable() {
        let tmp = TempDir::new().unwrap();
        run_with_dir(tmp.path(), flags(false, false, false)).unwrap();

        let cfg = ur_config::Config::load_from(tmp.path()).unwrap();
        assert_eq!(cfg.config_dir, tmp.path());
        assert_eq!(cfg.daemon_port, ur_config::DEFAULT_DAEMON_PORT);
    }
}
