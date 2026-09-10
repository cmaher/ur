use std::fs;
use std::fs::OpenOptions;
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use tracing::{debug, info, instrument};

use crate::output::OutputManager;

const EXAMPLE_LUA: &str = "\
-- Example hostexec Lua transform script.
--
-- Reference this from ur.toml:
--   [hostexec.commands]
--   mycommand = { lua = \"example.lua\" }
--
-- The transform function validates and optionally modifies the execution spec
-- before the command is executed on the host. Return a table with the full
-- execution spec, or call error() to block execution.
--
-- Parameters:
--   command       (string) - the command name (e.g. \"cargo\", \"make\")
--   args          (table)  - array of argument strings
--   working_dir   (string) - host-mapped working directory
--   worker_context (table|nil) - per-worker metadata when running in a project:
--     .worker_id   (string) - unique worker identifier (e.g. \"deploy-x7q2\")
--     .project_key (string) - project key from ur.toml (e.g. \"ur\")
--     .slot_path   (string) - host-side repo pool slot path
--
-- Returns a table with:
--   command     (string)           - command to execute (required)
--   args        (table)            - array of argument strings (required)
--   working_dir (string)           - working directory for the command (required)
--   env         (table|nil)        - string->string env vars added to the process (optional)
function transform(command, args, working_dir, worker_context)
    return {
        command = command,
        args = args,
        working_dir = working_dir,
        -- env = { MY_VAR = \"value\" },
    }
end
";

/// Domains seeded into `$UR_CONFIG/squid/allowlist.txt`, the file Squid actually
/// reads (mounted at `/etc/squid/allowlist.txt`, see `compose.rs`).
///
/// One shared Squid instance serves every worker regardless of which agent it
/// runs, so this must cover every agent in `AgentType::ALL` — a domain missing
/// here is a worker that boots healthy and then cannot reach its API.
/// Deliberately a literal rather than a fold over `AgentType::proxy_domains()`:
/// `ur init` writes this once and never rewrites it, so an existing config dir
/// keeps whatever it was seeded with either way. Adding an agent means adding
/// its domains in both places (here and `proxy_domains()`), and existing
/// installs need `ur init --force-squid` or `ur proxy allow <domain>`.
const DEFAULT_ALLOWLIST: &str = "\
api.anthropic.com
platform.claude.com
downloads.claude.ai
mcp-proxy.anthropic.com
chatgpt.com
api.openai.com
auth.openai.com
";

pub struct InitFlags {
    pub force: bool,
    pub force_config: bool,
    pub force_squid: bool,
}

#[instrument(skip(flags, output), fields(force = flags.force, force_config = flags.force_config, force_squid = flags.force_squid))]
pub fn run(flags: InitFlags, output: &OutputManager) -> Result<()> {
    let config_dir = ur_config::resolve_config_dir()?;
    info!(config_dir = %config_dir.display(), "initializing config directory");
    run_in(config_dir, flags, output)
}

#[instrument(skip(flags, output), fields(config_dir = %config_dir.display()))]
fn run_in(config_dir: PathBuf, flags: InitFlags, output: &OutputManager) -> Result<()> {
    init_dir(&config_dir, output)?;

    let workspace_dir = config_dir.join("workspace");
    init_dir(&workspace_dir, output)?;

    let squid_dir = config_dir.join("squid");
    init_dir(&squid_dir, output)?;

    for agent in [ur_config::AgentType::Claude, ur_config::AgentType::Agy] {
        init_dir(&config_dir.join(agent.name()), output)?;
    }

    let hostexec_dir = config_dir.join(ur_config::HOSTEXEC_DIR);
    init_dir(&hostexec_dir, output)?;

    let backup_dir = config_dir.join("backups");
    init_dir(&backup_dir, output)?;

    let should_force_config = flags.force || flags.force_config;
    let should_force_squid = flags.force || flags.force_squid;

    let default_toml = default_ur_toml(&config_dir);
    write_file(
        &config_dir.join("ur.toml"),
        &default_toml,
        should_force_config,
        "--force or --force-config",
        output,
    )?;
    write_file(
        &squid_dir.join("allowlist.txt"),
        DEFAULT_ALLOWLIST,
        should_force_squid,
        "--force or --force-squid",
        output,
    )?;

    write_file(
        &hostexec_dir.join("example.lua"),
        EXAMPLE_LUA,
        false,
        "--force",
        output,
    )?;

    // Load config to resolve logs_dir (may be customized in ur.toml).
    let config = ur_config::Config::load_from(&config_dir)?;
    init_dir(&config.logs_dir, output)?;

    // Credentials file must exist on the host for Docker file mounts to work
    // (otherwise Docker creates a directory at the mount path).
    for agent in [ur_config::AgentType::Claude, ur_config::AgentType::Agy] {
        init_credentials_file(&config_dir, agent, output)?;
    }

    Ok(())
}

fn init_credentials_file(
    config_dir: &Path,
    agent: ur_config::AgentType,
    output: &OutputManager,
) -> Result<()> {
    let credentials_path = agent
        .auth()
        .expect("credential file initialization requires an auth profile")
        .credentials_path;
    let filename = Path::new(credentials_path)
        .file_name()
        .expect("credentials_path has a filename");
    let path = config_dir.join(agent.name()).join(filename);
    if path.exists() {
        debug!(path = %path.display(), "skipping existing credentials file");
        return Ok(());
    }

    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&path)
        .with_context(|| format!("failed to create {}", path.display()))?;
    file.flush()
        .with_context(|| format!("failed to flush {}", path.display()))?;
    output.print_text(&format!("Created {}", path.display()));
    Ok(())
}

fn default_ur_toml(config_dir: &Path) -> String {
    let backup_dir = config_dir.join("backups");
    format!(
        "[backup]\npath = \"{}\"\ninterval_minutes = {}\n",
        backup_dir.display(),
        ur_config::DEFAULT_BACKUP_INTERVAL_MINUTES,
    )
}

fn init_dir(path: &Path, output: &OutputManager) -> Result<()> {
    debug!(path = %path.display(), "creating directory");
    fs::create_dir_all(path)
        .with_context(|| format!("failed to create directory {}", path.display()))?;
    output.print_text(&format!("Created {}", path.display()));
    Ok(())
}

fn write_file(
    path: &PathBuf,
    content: &str,
    force: bool,
    force_hint: &str,
    output: &OutputManager,
) -> Result<()> {
    if path.exists() && !force {
        debug!(path = %path.display(), "skipping existing file");
        output.print_text(&format!(
            "Skipped {} (exists, use {} to overwrite)",
            path.display(),
            force_hint
        ));
        return Ok(());
    }
    debug!(path = %path.display(), force, "writing file");
    fs::write(path, content).with_context(|| format!("failed to write {}", path.display()))?;
    output.print_text(&format!("Created {}", path.display()));
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

    fn text_output() -> OutputManager {
        OutputManager::from_args(Some("text"))
    }

    fn run_with_dir(dir: &Path, f: InitFlags) -> Result<()> {
        run_in(dir.to_path_buf(), f, &text_output())
    }

    #[test]
    fn creates_all_files_and_dirs() {
        let tmp = TempDir::new().unwrap();
        run_with_dir(tmp.path(), flags(false, false, false)).unwrap();

        assert!(tmp.path().join("workspace").is_dir());
        assert!(tmp.path().join("backups").is_dir());
        assert!(tmp.path().join("squid").is_dir());
        assert!(tmp.path().join("hostexec").is_dir());
        assert!(tmp.path().join("hostexec/example.lua").exists());
        assert!(tmp.path().join("logs").is_dir());
        assert!(tmp.path().join("ur.toml").exists());
        assert!(tmp.path().join("squid/allowlist.txt").exists());
        assert!(tmp.path().join("agy").is_dir());
        let agy_token = tmp.path().join("agy/antigravity-oauth-token");
        assert_eq!(fs::metadata(&agy_token).unwrap().len(), 0);
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(agy_token).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    #[test]
    fn ur_toml_has_backup_section() {
        let tmp = TempDir::new().unwrap();
        run_with_dir(tmp.path(), flags(false, false, false)).unwrap();

        let content = fs::read_to_string(tmp.path().join("ur.toml")).unwrap();
        assert!(content.contains("[backup]"));
        assert!(content.contains("interval_minutes = 30"));
        let expected_path = tmp.path().join("backups");
        assert!(content.contains(&format!("path = \"{}\"", expected_path.display())));
        assert!(
            !content.contains("node_id"),
            "node_id should not be in generated toml"
        );
    }

    #[test]
    fn allowlist_contains_anthropic() {
        let tmp = TempDir::new().unwrap();
        run_with_dir(tmp.path(), flags(false, false, false)).unwrap();

        let content = fs::read_to_string(tmp.path().join("squid/allowlist.txt")).unwrap();
        assert!(content.contains("api.anthropic.com"));
        assert!(content.contains("platform.claude.com"));
        assert!(content.contains("mcp-proxy.anthropic.com"));
    }

    /// The seeded `allowlist.txt` is what Squid actually reads, and one Squid
    /// instance serves every worker — so a domain any agent needs but this file
    /// omits is a worker that boots healthy and then can't reach its API.
    #[test]
    fn allowlist_covers_every_agents_proxy_domains() {
        let tmp = TempDir::new().unwrap();
        run_with_dir(tmp.path(), flags(false, false, false)).unwrap();

        let content = fs::read_to_string(tmp.path().join("squid/allowlist.txt")).unwrap();
        let seeded: Vec<&str> = content.lines().map(str::trim).collect();
        for agent in ur_config::AgentType::ALL {
            for domain in agent.proxy_domains() {
                assert!(
                    seeded.contains(domain),
                    "{} needs '{domain}' but DEFAULT_ALLOWLIST omits it: {seeded:?}",
                    agent.name()
                );
            }
        }
    }

    #[test]
    fn skips_existing_files_without_force() {
        let tmp = TempDir::new().unwrap();
        run_with_dir(tmp.path(), flags(false, false, false)).unwrap();

        // Modify a file to prove it won't be overwritten
        fs::write(
            tmp.path().join("ur.toml"),
            "node_id = \"n\"\nserver_port = 9999\n",
        )
        .unwrap();
        run_with_dir(tmp.path(), flags(false, false, false)).unwrap();

        let content = fs::read_to_string(tmp.path().join("ur.toml")).unwrap();
        assert_eq!(content, "node_id = \"n\"\nserver_port = 9999\n");
    }

    #[test]
    fn force_overwrites_all_files() {
        let tmp = TempDir::new().unwrap();
        run_with_dir(tmp.path(), flags(false, false, false)).unwrap();

        fs::write(tmp.path().join("ur.toml"), "server_port = 9999\n").unwrap();
        run_with_dir(tmp.path(), flags(true, false, false)).unwrap();

        let content = fs::read_to_string(tmp.path().join("ur.toml")).unwrap();
        assert!(content.contains("[backup]"), "should be reset to default");
        assert!(
            !content.contains("server_port"),
            "custom config should be gone"
        );
    }

    #[test]
    fn force_config_only_overwrites_toml() {
        let tmp = TempDir::new().unwrap();
        run_with_dir(tmp.path(), flags(false, false, false)).unwrap();

        fs::write(tmp.path().join("ur.toml"), "server_port = 9999\n").unwrap();
        run_with_dir(tmp.path(), flags(false, true, false)).unwrap();

        let toml_content = fs::read_to_string(tmp.path().join("ur.toml")).unwrap();
        assert!(
            toml_content.contains("[backup]"),
            "ur.toml should be overwritten with default"
        );
        assert!(
            !toml_content.contains("server_port"),
            "custom config should be gone"
        );
    }

    #[test]
    fn force_squid_overwrites_squid_dir() {
        let tmp = TempDir::new().unwrap();
        run_with_dir(tmp.path(), flags(false, false, false)).unwrap();

        fs::write(tmp.path().join("squid/allowlist.txt"), "custom.com\n").unwrap();
        fs::write(
            tmp.path().join("ur.toml"),
            "node_id = \"n\"\nserver_port = 9999\n",
        )
        .unwrap();
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
            toml_content, "node_id = \"n\"\nserver_port = 9999\n",
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
        assert_eq!(cfg.server_port, ur_config::DEFAULT_SERVER_PORT);
    }
}
