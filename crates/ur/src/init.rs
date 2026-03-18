use std::fs;
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

const DEFAULT_ALLOWLIST: &str = "\
api.anthropic.com
platform.claude.com
mcp-proxy.anthropic.com
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

    let claude_dir = config_dir.join(ur_config::CLAUDE_DIR);
    init_dir(&claude_dir, output)?;

    let hostexec_dir = config_dir.join(ur_config::HOSTEXEC_DIR);
    init_dir(&hostexec_dir, output)?;

    let backup_dir = config_dir.join("backups");
    init_dir(&backup_dir, output)?;

    let rag_dir = config_dir.join("rag");
    init_dir(&rag_dir, output)?;

    let rag_docs_dir = rag_dir.join("docs");
    init_dir(&rag_docs_dir, output)?;

    let rag_docs_rust_dir = rag_docs_dir.join("rust");
    init_dir(&rag_docs_rust_dir, output)?;

    let rag_qdrant_dir = rag_dir.join("qdrant");
    init_dir(&rag_qdrant_dir, output)?;

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

    // Credentials file must exist on the host for Docker file mounts to work
    // (otherwise Docker creates a directory at the mount path).
    write_file(
        &claude_dir.join(ur_config::CLAUDE_CREDENTIALS_FILENAME),
        "",
        false,
        "--force",
        output,
    )?;

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
        assert!(tmp.path().join("rag").is_dir());
        assert!(tmp.path().join("rag/docs").is_dir());
        assert!(tmp.path().join("rag/docs/rust").is_dir());
        assert!(tmp.path().join("rag/qdrant").is_dir());
        assert!(tmp.path().join("ur.toml").exists());
        assert!(tmp.path().join("squid/allowlist.txt").exists());
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

    #[test]
    fn skips_existing_files_without_force() {
        let tmp = TempDir::new().unwrap();
        run_with_dir(tmp.path(), flags(false, false, false)).unwrap();

        // Modify a file to prove it won't be overwritten
        fs::write(tmp.path().join("ur.toml"), "server_port = 9999\n").unwrap();
        run_with_dir(tmp.path(), flags(false, false, false)).unwrap();

        let content = fs::read_to_string(tmp.path().join("ur.toml")).unwrap();
        assert_eq!(content, "server_port = 9999\n");
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
        fs::write(tmp.path().join("ur.toml"), "server_port = 9999\n").unwrap();
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
            toml_content, "server_port = 9999\n",
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
