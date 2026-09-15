use std::collections::HashMap;
use std::path::Path;

use anyhow::Result;

use super::LuaDiscoveryManager;

#[derive(Debug, Clone)]
pub struct CommandConfig {
    pub lua_source: Option<String>,
    pub long_lived: bool,
    pub bidi: bool,
}

#[derive(Clone)]
pub struct HostExecConfigManager {
    commands: HashMap<String, CommandConfig>,
    discovery: Option<LuaDiscoveryManager>,
}

impl HostExecConfigManager {
    /// Build the baked-in defaults overlaid by discovered Lua scripts.
    pub fn load(config_dir: &Path, discovery: &LuaDiscoveryManager) -> Result<Self> {
        let mut commands = Self::defaults();
        let hostexec_dir = config_dir.join(ur_config::HOSTEXEC_DIR);
        for (name, command) in discovery.discover(&hostexec_dir)? {
            commands.insert(name, command);
        }

        Ok(Self {
            commands,
            discovery: Some(discovery.clone()),
        })
    }

    pub fn reload(&self, config_dir: &Path) -> Result<Self> {
        let discovery = self
            .discovery
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("hostexec discovery is not configured"))?;
        Self::load(config_dir, discovery)
    }

    /// Create an empty config manager with no commands.
    ///
    /// Useful for tests where hostexec is irrelevant.
    pub fn empty() -> Self {
        Self {
            commands: HashMap::new(),
            discovery: None,
        }
    }

    /// Create a new config manager containing only the built-in default commands.
    pub fn defaults_only(&self) -> Self {
        Self {
            commands: self.effective_defaults(),
            discovery: self.discovery.clone(),
        }
    }

    /// Create a new config with only default commands plus project-granted commands.
    ///
    /// Per-project `hostexec` arrays in `ur.toml` grant workers access to commands.
    /// Granted commands that exist in the discovered registry use
    /// their configured settings (lua, long_lived, bidi). Granted commands not in the
    /// registry are added as passthrough (no Lua, not long_lived, not bidi).
    /// Default commands (git, gh, cargo, docker, ur) are always included.
    pub fn with_project_commands(&self, granted: &[String]) -> Self {
        let mut commands = self.effective_defaults();
        for name in granted {
            if let Some(cfg) = self.commands.get(name) {
                commands.insert(name.clone(), cfg.clone());
            } else {
                commands.entry(name.clone()).or_insert(CommandConfig {
                    lua_source: None,
                    long_lived: false,
                    bidi: false,
                });
            }
        }
        Self {
            commands,
            discovery: self.discovery.clone(),
        }
    }

    fn effective_defaults(&self) -> HashMap<String, CommandConfig> {
        Self::defaults()
            .into_keys()
            .filter_map(|name| {
                self.commands
                    .get(&name)
                    .cloned()
                    .map(|config| (name, config))
            })
            .collect()
    }

    fn defaults() -> HashMap<String, CommandConfig> {
        let mut commands = HashMap::new();
        commands.insert(
            "git".into(),
            CommandConfig {
                lua_source: Some(include_str!("default_scripts/git.lua").into()),
                long_lived: false,
                bidi: false,
            },
        );
        commands.insert(
            "gh".into(),
            CommandConfig {
                lua_source: Some(include_str!("default_scripts/gh.lua").into()),
                long_lived: false,
                // Bidi so `gh api --input -` and `gh pr comment --body-file -`
                // can read forwarded stdin. gh runs on the host and cannot see
                // the worker filesystem, so stdin is the only way to hand it a
                // nested JSON body (e.g. a review's comments[] array).
                bidi: true,
            },
        );
        commands.insert(
            "cargo".into(),
            CommandConfig {
                lua_source: Some(include_str!("default_scripts/cargo.lua").into()),
                long_lived: false,
                bidi: false,
            },
        );
        commands.insert(
            "docker".into(),
            CommandConfig {
                lua_source: Some(include_str!("default_scripts/docker.lua").into()),
                long_lived: false,
                bidi: false,
            },
        );
        commands.insert(
            "ur".into(),
            CommandConfig {
                lua_source: Some(include_str!("default_scripts/ur.lua").into()),
                long_lived: false,
                bidi: false,
            },
        );
        commands.insert(
            "make".into(),
            CommandConfig {
                lua_source: Some(include_str!("default_scripts/make.lua").into()),
                long_lived: false,
                bidi: false,
            },
        );
        commands.insert(
            "go".into(),
            CommandConfig {
                lua_source: Some(include_str!("default_scripts/go.lua").into()),
                long_lived: false,
                bidi: false,
            },
        );
        commands.insert(
            "bazel".into(),
            CommandConfig {
                lua_source: Some(include_str!("default_scripts/bazel.lua").into()),
                long_lived: false,
                bidi: false,
            },
        );
        commands.insert(
            "npm".into(),
            CommandConfig {
                lua_source: Some(include_str!("default_scripts/npm.lua").into()),
                long_lived: false,
                bidi: false,
            },
        );
        commands.insert(
            "pnpm".into(),
            CommandConfig {
                lua_source: Some(include_str!("default_scripts/pnpm.lua").into()),
                long_lived: false,
                bidi: false,
            },
        );
        commands.insert(
            "tsc".into(),
            CommandConfig {
                lua_source: Some(include_str!("default_scripts/tsc.lua").into()),
                long_lived: false,
                bidi: false,
            },
        );
        commands
    }

    pub fn is_allowed(&self, command: &str) -> bool {
        self.commands.contains_key(command)
    }

    pub fn get(&self, command: &str) -> Option<&CommandConfig> {
        self.commands.get(command)
    }

    pub fn command_names(&self) -> Vec<String> {
        let mut names: Vec<String> = self.commands.keys().cloned().collect();
        names.sort();
        names
    }

    pub fn command_entries(&self) -> Vec<ur_rpc::proto::hostexec::HostExecCommandEntry> {
        let mut entries: Vec<_> = self
            .commands
            .iter()
            .map(
                |(name, cfg)| ur_rpc::proto::hostexec::HostExecCommandEntry {
                    name: name.clone(),
                    bidi: cfg.bidi,
                },
            )
            .collect();
        entries.sort_by(|a, b| a.name.cmp(&b.name));
        entries
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hostexec::lua_transform::{LuaTransformManager, WorkerContext};
    use std::fs;
    use std::path::PathBuf;
    use tempfile::TempDir;

    const TRANSFORM: &str = r#"
        function transform(command, args, working_dir)
            return { command = command, args = args, working_dir = working_dir }
        end
    "#;

    fn write_lua(temp: &TempDir, name: &str, source: &str) {
        let hostexec_dir = temp.path().join(ur_config::HOSTEXEC_DIR);
        fs::create_dir_all(&hostexec_dir).unwrap();
        fs::write(hostexec_dir.join(format!("{name}.lua")), source).unwrap();
    }

    fn load(temp: &TempDir) -> HostExecConfigManager {
        let discovery = LuaDiscoveryManager::new(LuaTransformManager::new());
        HostExecConfigManager::load(temp.path(), &discovery).unwrap()
    }

    #[test]
    fn load_includes_all_built_in_defaults() {
        let tmp = TempDir::new().unwrap();
        let mgr = load(&tmp);

        assert_eq!(
            mgr.command_names(),
            vec![
                "bazel", "cargo", "docker", "gh", "git", "go", "make", "npm", "pnpm", "tsc", "ur"
            ]
        );
    }

    #[test]
    fn discovered_script_shadows_built_in() {
        let tmp = TempDir::new().unwrap();
        let script = format!("-- user git transform\n{TRANSFORM}");
        write_lua(&tmp, "git", &script);

        let mgr = load(&tmp);

        assert_eq!(
            mgr.get("git").unwrap().lua_source.as_deref(),
            Some(script.as_str())
        );
    }

    #[test]
    fn discovered_script_shadows_built_in_for_project_without_explicit_grant() {
        let tmp = TempDir::new().unwrap();
        let script = format!("-- user git transform\n{TRANSFORM}");
        write_lua(&tmp, "git", &script);

        let mgr = load(&tmp);
        let project = mgr.with_project_commands(&[]);

        assert_eq!(
            project.get("git").unwrap().lua_source.as_deref(),
            Some(script.as_str())
        );
    }

    #[test]
    fn granted_discovered_command_uses_script_metadata() {
        let tmp = TempDir::new().unwrap();
        let script = format!("long_lived = true\nbidi = true\n{TRANSFORM}");
        write_lua(&tmp, "daemon", &script);

        let mgr = load(&tmp);
        let project = mgr.with_project_commands(&["daemon".into()]);
        let daemon = project.get("daemon").unwrap();

        assert_eq!(daemon.lua_source.as_deref(), Some(script.as_str()));
        assert!(daemon.long_lived);
        assert!(daemon.bidi);
    }

    #[test]
    fn granted_unknown_command_remains_passthrough() {
        let tmp = TempDir::new().unwrap();
        let mgr = load(&tmp);
        let project = mgr.with_project_commands(&["rg".into()]);
        let rg = project.get("rg").unwrap();

        assert!(rg.lua_source.is_none());
        assert!(!rg.long_lived);
        assert!(!rg.bidi);
    }

    #[test]
    fn discovered_command_requires_project_grant() {
        let tmp = TempDir::new().unwrap();
        write_lua(&tmp, "private", TRANSFORM);
        let mgr = load(&tmp);

        assert!(mgr.is_allowed("private"));
        assert!(!mgr.with_project_commands(&[]).is_allowed("private"));
    }

    #[test]
    fn defaults_only_excludes_discovered_commands() {
        let tmp = TempDir::new().unwrap();
        write_lua(&tmp, "private", TRANSFORM);
        let mgr = load(&tmp);

        assert!(mgr.defaults_only().is_allowed("git"));
        assert!(!mgr.defaults_only().is_allowed("private"));
    }

    /// gh must stay bidi: workerd only adds `--bidi` to the generated shim when
    /// this flag is set, and without it `gh api --input -` gets no stdin.
    #[test]
    fn test_gh_default_is_bidi() {
        let tmp = TempDir::new().unwrap();
        let mgr = load(&tmp);
        let gh_cfg = mgr.get("gh").unwrap();
        assert!(gh_cfg.bidi, "gh must be bidi so --input - receives stdin");
        assert!(!gh_cfg.long_lived);
    }

    // --- npm.lua tests ---

    fn npm_script() -> &'static str {
        include_str!("default_scripts/npm.lua")
    }

    fn npm_worker_context() -> WorkerContext {
        WorkerContext {
            worker_id: "worker-1".into(),
            process_id: "ur-abc12".into(),
            project_key: "myproject".into(),
            slot_path: PathBuf::from("/home/user/.ur/workspace/pool/myproject/0"),
            branch: "feature-branch".into(),
        }
    }

    fn run_npm(args: &[&str], ctx: Option<&WorkerContext>) -> anyhow::Result<Vec<String>> {
        let mgr = LuaTransformManager::new();
        let string_args: Vec<String> = args.iter().map(|s| (*s).to_string()).collect();
        let result = mgr.run_transform(npm_script(), "npm", &string_args, "/workspace", ctx)?;
        Ok(result.args)
    }

    // Blocked subcommands

    #[test]
    fn test_npm_blocks_add() {
        let err = run_npm(&["add", "react"], None).unwrap_err();
        assert!(err.to_string().contains("blocked npm subcommand: add"));
    }

    #[test]
    fn test_npm_blocks_uninstall() {
        let err = run_npm(&["uninstall", "react"], None).unwrap_err();
        assert!(
            err.to_string()
                .contains("blocked npm subcommand: uninstall")
        );
    }

    #[test]
    fn test_npm_blocks_remove() {
        let err = run_npm(&["remove", "react"], None).unwrap_err();
        assert!(err.to_string().contains("blocked npm subcommand: remove"));
    }

    #[test]
    fn test_npm_blocks_rm() {
        let err = run_npm(&["rm", "react"], None).unwrap_err();
        assert!(err.to_string().contains("blocked npm subcommand: rm"));
    }

    #[test]
    fn test_npm_blocks_un() {
        let err = run_npm(&["un", "react"], None).unwrap_err();
        assert!(err.to_string().contains("blocked npm subcommand: un"));
    }

    #[test]
    fn test_npm_blocks_unlink() {
        let err = run_npm(&["unlink"], None).unwrap_err();
        assert!(err.to_string().contains("blocked npm subcommand: unlink"));
    }

    #[test]
    fn test_npm_blocks_publish() {
        let err = run_npm(&["publish"], None).unwrap_err();
        assert!(err.to_string().contains("blocked npm subcommand: publish"));
    }

    #[test]
    fn test_npm_blocks_login() {
        let err = run_npm(&["login"], None).unwrap_err();
        assert!(err.to_string().contains("blocked npm subcommand: login"));
    }

    #[test]
    fn test_npm_blocks_logout() {
        let err = run_npm(&["logout"], None).unwrap_err();
        assert!(err.to_string().contains("blocked npm subcommand: logout"));
    }

    #[test]
    fn test_npm_blocks_adduser() {
        let err = run_npm(&["adduser"], None).unwrap_err();
        assert!(err.to_string().contains("blocked npm subcommand: adduser"));
    }

    #[test]
    fn test_npm_blocks_config() {
        let err = run_npm(&["config", "set", "key", "val"], None).unwrap_err();
        assert!(err.to_string().contains("blocked npm subcommand: config"));
    }

    #[test]
    fn test_npm_blocks_set() {
        let err = run_npm(&["set", "key=val"], None).unwrap_err();
        assert!(err.to_string().contains("blocked npm subcommand: set"));
    }

    #[test]
    fn test_npm_blocks_get() {
        let err = run_npm(&["get", "key"], None).unwrap_err();
        assert!(err.to_string().contains("blocked npm subcommand: get"));
    }

    #[test]
    fn test_npm_blocks_version() {
        let err = run_npm(&["version", "patch"], None).unwrap_err();
        assert!(err.to_string().contains("blocked npm subcommand: version"));
    }

    #[test]
    fn test_npm_blocks_link() {
        let err = run_npm(&["link"], None).unwrap_err();
        assert!(err.to_string().contains("blocked npm subcommand: link"));
    }

    #[test]
    fn test_npm_blocks_token() {
        let err = run_npm(&["token", "list"], None).unwrap_err();
        assert!(err.to_string().contains("blocked npm subcommand: token"));
    }

    #[test]
    fn test_npm_blocks_owner() {
        let err = run_npm(&["owner", "add", "user", "pkg"], None).unwrap_err();
        assert!(err.to_string().contains("blocked npm subcommand: owner"));
    }

    #[test]
    fn test_npm_blocks_profile() {
        let err = run_npm(&["profile", "get"], None).unwrap_err();
        assert!(err.to_string().contains("blocked npm subcommand: profile"));
    }

    #[test]
    fn test_npm_blocks_team() {
        let err = run_npm(&["team", "ls", "org"], None).unwrap_err();
        assert!(err.to_string().contains("blocked npm subcommand: team"));
    }

    #[test]
    fn test_npm_blocks_org() {
        let err = run_npm(&["org", "set", "org", "user"], None).unwrap_err();
        assert!(err.to_string().contains("blocked npm subcommand: org"));
    }

    #[test]
    fn test_npm_blocks_hook() {
        let err = run_npm(&["hook", "ls"], None).unwrap_err();
        assert!(err.to_string().contains("blocked npm subcommand: hook"));
    }

    #[test]
    fn test_npm_blocks_access() {
        let err = run_npm(&["access", "public", "pkg"], None).unwrap_err();
        assert!(err.to_string().contains("blocked npm subcommand: access"));
    }

    #[test]
    fn test_npm_blocks_deprecate() {
        let err = run_npm(&["deprecate", "pkg@1.0.0", "msg"], None).unwrap_err();
        assert!(
            err.to_string()
                .contains("blocked npm subcommand: deprecate")
        );
    }

    #[test]
    fn test_npm_blocks_dist_tag() {
        let err = run_npm(&["dist-tag", "add", "pkg@1.0.0", "latest"], None).unwrap_err();
        assert!(err.to_string().contains("blocked npm subcommand: dist-tag"));
    }

    #[test]
    fn test_npm_blocks_star() {
        let err = run_npm(&["star", "pkg"], None).unwrap_err();
        assert!(err.to_string().contains("blocked npm subcommand: star"));
    }

    #[test]
    fn test_npm_blocks_unstar() {
        let err = run_npm(&["unstar", "pkg"], None).unwrap_err();
        assert!(err.to_string().contains("blocked npm subcommand: unstar"));
    }

    #[test]
    fn test_npm_blocks_init() {
        let err = run_npm(&["init", "react-app"], None).unwrap_err();
        assert!(err.to_string().contains("blocked npm subcommand: init"));
    }

    #[test]
    fn test_npm_blocks_rebuild() {
        let err = run_npm(&["rebuild"], None).unwrap_err();
        assert!(err.to_string().contains("blocked npm subcommand: rebuild"));
    }

    // Allowed subcommands

    #[test]
    fn test_npm_allows_install() {
        let args = run_npm(&["install"], None).unwrap();
        assert_eq!(args, vec!["install"]);
    }

    #[test]
    fn test_npm_allows_install_save_exact() {
        let args = run_npm(&["install", "--save-exact"], None).unwrap();
        assert_eq!(args, vec!["install", "--save-exact"]);
    }

    #[test]
    fn test_npm_allows_install_with_package() {
        let args = run_npm(&["install", "react"], None).unwrap();
        assert_eq!(args, vec!["install", "react"]);
    }

    #[test]
    fn test_npm_allows_ci() {
        let args = run_npm(&["ci"], None).unwrap();
        assert_eq!(args, vec!["ci"]);
    }

    #[test]
    fn test_npm_allows_run_build() {
        let args = run_npm(&["run", "build"], None).unwrap();
        assert_eq!(args, vec!["run", "build"]);
    }

    #[test]
    fn test_npm_allows_run_lint() {
        let args = run_npm(&["run", "lint"], None).unwrap();
        assert_eq!(args, vec!["run", "lint"]);
    }

    #[test]
    fn test_npm_allows_test() {
        let args = run_npm(&["test"], None).unwrap();
        assert_eq!(args, vec!["test"]);
    }

    #[test]
    fn test_npm_allows_exec_turbo_build() {
        let args = run_npm(&["exec", "turbo", "build"], None).unwrap();
        assert_eq!(args, vec!["exec", "turbo", "build"]);
    }

    // --prefix rewriting

    #[test]
    fn test_npm_prefix_rewrite_workspace() {
        let ctx = npm_worker_context();
        let args = run_npm(&["--prefix", "/workspace", "install"], Some(&ctx)).unwrap();
        assert_eq!(
            args,
            vec![
                "--prefix",
                "/home/user/.ur/workspace/pool/myproject/0",
                "install"
            ]
        );
    }

    #[test]
    fn test_npm_prefix_rewrite_project_key() {
        let ctx = npm_worker_context();
        let args = run_npm(&["--prefix", "myproject", "install"], Some(&ctx)).unwrap();
        assert_eq!(
            args,
            vec![
                "--prefix",
                "/home/user/.ur/workspace/pool/myproject/0",
                "install"
            ]
        );
    }

    #[test]
    fn test_npm_prefix_blocks_wrong_absolute_path() {
        let ctx = npm_worker_context();
        let err = run_npm(&["--prefix", "/other/path", "install"], Some(&ctx)).unwrap_err();
        assert!(err.to_string().contains("does not match project key"));
    }

    #[test]
    fn test_npm_prefix_blocks_without_worker_context() {
        let err = run_npm(&["--prefix", "/workspace", "install"], None).unwrap_err();
        assert!(err.to_string().contains("blocked flag: --prefix"));
    }

    #[test]
    fn test_npm_prefix_equals_form_rejected() {
        let err = run_npm(&["--prefix=/workspace", "install"], None).unwrap_err();
        assert!(err.to_string().contains("--prefix=<path>"));
    }

    #[test]
    fn test_npm_prefix_relative_passthrough() {
        let ctx = npm_worker_context();
        let args = run_npm(&["--prefix", "workspace", "install"], Some(&ctx)).unwrap();
        assert_eq!(
            args,
            vec![
                "--prefix",
                "/home/user/.ur/workspace/pool/myproject/0",
                "install"
            ]
        );
    }

    // --cwd rewriting

    #[test]
    fn test_npm_cwd_rewrite_workspace() {
        let ctx = npm_worker_context();
        let args = run_npm(&["--cwd", "/workspace", "install"], Some(&ctx)).unwrap();
        assert_eq!(
            args,
            vec![
                "--cwd",
                "/home/user/.ur/workspace/pool/myproject/0",
                "install"
            ]
        );
    }

    #[test]
    fn test_npm_cwd_rewrite_project_key() {
        let ctx = npm_worker_context();
        let args = run_npm(&["--cwd", "myproject", "install"], Some(&ctx)).unwrap();
        assert_eq!(
            args,
            vec![
                "--cwd",
                "/home/user/.ur/workspace/pool/myproject/0",
                "install"
            ]
        );
    }

    #[test]
    fn test_npm_cwd_blocks_wrong_absolute_path() {
        let ctx = npm_worker_context();
        let err = run_npm(&["--cwd", "/other/path", "install"], Some(&ctx)).unwrap_err();
        assert!(err.to_string().contains("does not match project key"));
    }

    #[test]
    fn test_npm_cwd_blocks_without_worker_context() {
        let err = run_npm(&["--cwd", "/workspace", "install"], None).unwrap_err();
        assert!(err.to_string().contains("blocked flag: --cwd"));
    }

    #[test]
    fn test_npm_cwd_equals_form_rejected() {
        let err = run_npm(&["--cwd=/workspace", "install"], None).unwrap_err();
        assert!(err.to_string().contains("--cwd=<path>"));
    }

    #[test]
    fn test_npm_cwd_relative_passthrough() {
        let ctx = npm_worker_context();
        let args = run_npm(&["--cwd", "workspace", "install"], Some(&ctx)).unwrap();
        assert_eq!(
            args,
            vec![
                "--cwd",
                "/home/user/.ur/workspace/pool/myproject/0",
                "install"
            ]
        );
    }

    // Blocked exact flags

    #[test]
    fn test_npm_blocks_dash_g() {
        let err = run_npm(&["-g", "install", "foo"], None).unwrap_err();
        assert!(err.to_string().contains("blocked flag: -g"));
    }

    #[test]
    fn test_npm_blocks_global() {
        let err = run_npm(&["--global", "install", "foo"], None).unwrap_err();
        assert!(err.to_string().contains("blocked flag: --global"));
    }

    #[test]
    fn test_npm_blocks_install_dash_g_pkg() {
        let err = run_npm(&["install", "-g", "react"], None).unwrap_err();
        assert!(err.to_string().contains("blocked flag: -g"));
    }

    #[test]
    fn test_npm_blocks_install_global_pkg() {
        let err = run_npm(&["install", "--global", "react"], None).unwrap_err();
        assert!(err.to_string().contains("blocked flag: --global"));
    }

    // Blocked flag prefixes

    #[test]
    fn test_npm_blocks_cache_flag() {
        let err = run_npm(&["--cache", "/tmp"], None).unwrap_err();
        assert!(err.to_string().contains("blocked flag: --cache"));
    }

    #[test]
    fn test_npm_blocks_cache_equals_form() {
        let err = run_npm(&["--cache=/tmp"], None).unwrap_err();
        assert!(err.to_string().contains("blocked flag: --cache=/tmp"));
    }

    #[test]
    fn test_npm_blocks_globalconfig() {
        let err = run_npm(&["--globalconfig", "/tmp/.npmrc"], None).unwrap_err();
        assert!(err.to_string().contains("blocked flag: --globalconfig"));
    }

    #[test]
    fn test_npm_blocks_userconfig() {
        let err = run_npm(&["--userconfig", "/tmp/.npmrc"], None).unwrap_err();
        assert!(err.to_string().contains("blocked flag: --userconfig"));
    }

    #[test]
    fn test_npm_blocks_location_equals_global() {
        let err = run_npm(&["--location=global"], None).unwrap_err();
        assert!(err.to_string().contains("blocked flag: --location=global"));
    }

    #[test]
    fn test_npm_blocks_location_user() {
        let err = run_npm(&["--location", "user"], None).unwrap_err();
        assert!(err.to_string().contains("blocked flag: --location"));
    }

    // Passthrough flags

    #[test]
    fn test_npm_passthrough_save() {
        let args = run_npm(&["install", "--save"], None).unwrap();
        assert_eq!(args, vec!["install", "--save"]);
    }

    #[test]
    fn test_npm_passthrough_save_dev() {
        let args = run_npm(&["install", "--save-dev"], None).unwrap();
        assert_eq!(args, vec!["install", "--save-dev"]);
    }

    #[test]
    fn test_npm_passthrough_workspace_equals() {
        let args = run_npm(&["install", "--workspace=foo"], None).unwrap();
        assert_eq!(args, vec!["install", "--workspace=foo"]);
    }

    #[test]
    fn test_npm_passthrough_workspace_short() {
        let args = run_npm(&["install", "-w", "foo"], None).unwrap();
        assert_eq!(args, vec!["install", "-w", "foo"]);
    }

    #[test]
    fn test_npm_passthrough_workspaces() {
        let args = run_npm(&["install", "--workspaces"], None).unwrap();
        assert_eq!(args, vec!["install", "--workspaces"]);
    }

    // --- pnpm.lua tests ---

    fn pnpm_script() -> &'static str {
        include_str!("default_scripts/pnpm.lua")
    }

    fn pnpm_worker_context() -> WorkerContext {
        WorkerContext {
            worker_id: "worker-1".into(),
            process_id: "ur-abc12".into(),
            project_key: "myproject".into(),
            slot_path: PathBuf::from("/home/user/.ur/workspace/pool/myproject/0"),
            branch: "feature-branch".into(),
        }
    }

    fn run_pnpm(args: &[&str], ctx: Option<&WorkerContext>) -> anyhow::Result<Vec<String>> {
        let mgr = LuaTransformManager::new();
        let string_args: Vec<String> = args.iter().map(|s| (*s).to_string()).collect();
        let result = mgr.run_transform(pnpm_script(), "pnpm", &string_args, "/workspace", ctx)?;
        Ok(result.args)
    }

    // Blocked subcommands

    #[test]
    fn test_pnpm_blocks_add() {
        let err = run_pnpm(&["add", "react"], None).unwrap_err();
        assert!(err.to_string().contains("blocked pnpm subcommand: add"));
    }

    #[test]
    fn test_pnpm_blocks_remove() {
        let err = run_pnpm(&["remove", "react"], None).unwrap_err();
        assert!(err.to_string().contains("blocked pnpm subcommand: remove"));
    }

    #[test]
    fn test_pnpm_blocks_dlx() {
        let err = run_pnpm(&["dlx", "create-react-app"], None).unwrap_err();
        assert!(err.to_string().contains("blocked pnpm subcommand: dlx"));
    }

    #[test]
    fn test_pnpm_blocks_create() {
        let err = run_pnpm(&["create", "react-app"], None).unwrap_err();
        assert!(err.to_string().contains("blocked pnpm subcommand: create"));
    }

    #[test]
    fn test_pnpm_blocks_publish() {
        let err = run_pnpm(&["publish"], None).unwrap_err();
        assert!(err.to_string().contains("blocked pnpm subcommand: publish"));
    }

    #[test]
    fn test_pnpm_blocks_login() {
        let err = run_pnpm(&["login"], None).unwrap_err();
        assert!(err.to_string().contains("blocked pnpm subcommand: login"));
    }

    #[test]
    fn test_pnpm_blocks_logout() {
        let err = run_pnpm(&["logout"], None).unwrap_err();
        assert!(err.to_string().contains("blocked pnpm subcommand: logout"));
    }

    #[test]
    fn test_pnpm_blocks_config() {
        let err = run_pnpm(&["config", "set", "key", "val"], None).unwrap_err();
        assert!(err.to_string().contains("blocked pnpm subcommand: config"));
    }

    #[test]
    fn test_pnpm_blocks_setup() {
        let err = run_pnpm(&["setup"], None).unwrap_err();
        assert!(err.to_string().contains("blocked pnpm subcommand: setup"));
    }

    #[test]
    fn test_pnpm_blocks_env() {
        let err = run_pnpm(&["env", "use", "18"], None).unwrap_err();
        assert!(err.to_string().contains("blocked pnpm subcommand: env"));
    }

    #[test]
    fn test_pnpm_blocks_server() {
        let err = run_pnpm(&["server", "start"], None).unwrap_err();
        assert!(err.to_string().contains("blocked pnpm subcommand: server"));
    }

    #[test]
    fn test_pnpm_blocks_store() {
        let err = run_pnpm(&["store", "prune"], None).unwrap_err();
        assert!(err.to_string().contains("blocked pnpm subcommand: store"));
    }

    #[test]
    fn test_pnpm_blocks_patch() {
        let err = run_pnpm(&["patch", "react"], None).unwrap_err();
        assert!(err.to_string().contains("blocked pnpm subcommand: patch"));
    }

    #[test]
    fn test_pnpm_blocks_patch_commit() {
        let err = run_pnpm(&["patch-commit", "/tmp/patch"], None).unwrap_err();
        assert!(
            err.to_string()
                .contains("blocked pnpm subcommand: patch-commit")
        );
    }

    #[test]
    fn test_pnpm_blocks_rebuild() {
        let err = run_pnpm(&["rebuild"], None).unwrap_err();
        assert!(err.to_string().contains("blocked pnpm subcommand: rebuild"));
    }

    #[test]
    fn test_pnpm_blocks_deploy() {
        let err = run_pnpm(&["deploy", "--filter=app", "/tmp/out"], None).unwrap_err();
        assert!(err.to_string().contains("blocked pnpm subcommand: deploy"));
    }

    // Allowed subcommands

    #[test]
    fn test_pnpm_allows_install() {
        let args = run_pnpm(&["install"], None).unwrap();
        assert_eq!(args, vec!["install"]);
    }

    #[test]
    fn test_pnpm_allows_install_frozen_lockfile() {
        let args = run_pnpm(&["install", "--frozen-lockfile"], None).unwrap();
        assert_eq!(args, vec!["install", "--frozen-lockfile"]);
    }

    #[test]
    fn test_pnpm_allows_run_compile() {
        let args = run_pnpm(&["run", "compile"], None).unwrap();
        assert_eq!(args, vec!["run", "compile"]);
    }

    #[test]
    fn test_pnpm_allows_run_lint() {
        let args = run_pnpm(&["run", "lint"], None).unwrap();
        assert_eq!(args, vec!["run", "lint"]);
    }

    #[test]
    fn test_pnpm_allows_test() {
        let args = run_pnpm(&["test"], None).unwrap();
        assert_eq!(args, vec!["test"]);
    }

    #[test]
    fn test_pnpm_allows_exec() {
        let args = run_pnpm(&["exec", "turbo", "build"], None).unwrap();
        assert_eq!(args, vec!["exec", "turbo", "build"]);
    }

    #[test]
    fn test_pnpm_allows_bare_script_compile() {
        let args = run_pnpm(&["compile"], None).unwrap();
        assert_eq!(args, vec!["compile"]);
    }

    #[test]
    fn test_pnpm_allows_bare_script_lint() {
        let args = run_pnpm(&["lint"], None).unwrap();
        assert_eq!(args, vec!["lint"]);
    }

    #[test]
    fn test_pnpm_allows_prettier() {
        let args = run_pnpm(&["prettier", "--write", "."], None).unwrap();
        assert_eq!(args, vec!["prettier", "--write", "."]);
    }

    #[test]
    fn test_pnpm_allows_turbo_with_filter() {
        let args = run_pnpm(
            &[
                "turbo",
                "--filter=@acme/admin",
                "build:e2e",
                "compile",
                "lint",
                "prettier:ci",
            ],
            None,
        )
        .unwrap();
        assert_eq!(
            args,
            vec![
                "turbo",
                "--filter=@acme/admin",
                "build:e2e",
                "compile",
                "lint",
                "prettier:ci"
            ]
        );
    }

    // -C rewriting with worker_context

    #[test]
    fn test_pnpm_dash_c_rewrite_workspace() {
        let ctx = pnpm_worker_context();
        let args = run_pnpm(&["-C", "/workspace", "install"], Some(&ctx)).unwrap();
        assert_eq!(
            args,
            vec!["-C", "/home/user/.ur/workspace/pool/myproject/0", "install"]
        );
    }

    #[test]
    fn test_pnpm_dash_c_rewrite_project_key() {
        let ctx = pnpm_worker_context();
        let args = run_pnpm(&["-C", "myproject", "install"], Some(&ctx)).unwrap();
        assert_eq!(
            args,
            vec!["-C", "/home/user/.ur/workspace/pool/myproject/0", "install"]
        );
    }

    #[test]
    fn test_pnpm_dash_c_blocks_wrong_path() {
        let ctx = pnpm_worker_context();
        let err = run_pnpm(&["-C", "/other/path", "install"], Some(&ctx)).unwrap_err();
        assert!(err.to_string().contains("does not match project key"));
    }

    #[test]
    fn test_pnpm_dash_c_blocks_without_worker_context() {
        let err = run_pnpm(&["-C", "/workspace", "install"], None).unwrap_err();
        assert!(err.to_string().contains("blocked flag: -C"));
    }

    // --dir rewriting

    #[test]
    fn test_pnpm_dir_rewrite_workspace() {
        let ctx = pnpm_worker_context();
        let args = run_pnpm(&["--dir", "/workspace", "install"], Some(&ctx)).unwrap();
        assert_eq!(
            args,
            vec![
                "--dir",
                "/home/user/.ur/workspace/pool/myproject/0",
                "install"
            ]
        );
    }

    #[test]
    fn test_pnpm_dir_rewrite_project_key() {
        let ctx = pnpm_worker_context();
        let args = run_pnpm(&["--dir", "myproject", "install"], Some(&ctx)).unwrap();
        assert_eq!(
            args,
            vec![
                "--dir",
                "/home/user/.ur/workspace/pool/myproject/0",
                "install"
            ]
        );
    }

    #[test]
    fn test_pnpm_dir_blocks_wrong_path() {
        let ctx = pnpm_worker_context();
        let err = run_pnpm(&["--dir", "/other/path", "install"], Some(&ctx)).unwrap_err();
        assert!(err.to_string().contains("does not match project key"));
    }

    #[test]
    fn test_pnpm_dir_blocks_without_worker_context() {
        let err = run_pnpm(&["--dir", "/workspace", "install"], None).unwrap_err();
        assert!(err.to_string().contains("blocked flag: --dir"));
    }

    #[test]
    fn test_pnpm_dir_equals_form_rejected() {
        let err = run_pnpm(&["--dir=/workspace", "install"], None).unwrap_err();
        assert!(err.to_string().contains("--dir=<path>"));
    }

    // Blocked flag prefixes

    #[test]
    fn test_pnpm_blocks_global_dir() {
        let err = run_pnpm(&["--global-dir", "/tmp"], None).unwrap_err();
        assert!(err.to_string().contains("blocked flag: --global-dir"));
    }

    #[test]
    fn test_pnpm_blocks_global_dir_equals() {
        let err = run_pnpm(&["--global-dir=/tmp"], None).unwrap_err();
        assert!(err.to_string().contains("blocked flag: --global-dir=/tmp"));
    }

    #[test]
    fn test_pnpm_blocks_store_dir() {
        let err = run_pnpm(&["--store-dir", "/tmp"], None).unwrap_err();
        assert!(err.to_string().contains("blocked flag: --store-dir"));
    }

    #[test]
    fn test_pnpm_blocks_store_dir_equals() {
        let err = run_pnpm(&["--store-dir=/tmp"], None).unwrap_err();
        assert!(err.to_string().contains("blocked flag: --store-dir=/tmp"));
    }

    #[test]
    fn test_pnpm_blocks_dash_g() {
        let err = run_pnpm(&["-g", "install", "foo"], None).unwrap_err();
        assert!(err.to_string().contains("blocked flag: -g"));
    }

    #[test]
    fn test_pnpm_blocks_global() {
        let err = run_pnpm(&["--global", "install", "foo"], None).unwrap_err();
        assert!(err.to_string().contains("blocked flag: --global"));
    }

    #[test]
    fn test_pnpm_blocks_install_dash_g_pkg() {
        let err = run_pnpm(&["install", "-g", "react"], None).unwrap_err();
        assert!(err.to_string().contains("blocked flag: -g"));
    }

    #[test]
    fn test_pnpm_blocks_install_global_pkg() {
        let err = run_pnpm(&["install", "--global", "react"], None).unwrap_err();
        assert!(err.to_string().contains("blocked flag: --global"));
    }

    #[test]
    fn test_pnpm_blocks_global_bin_dir() {
        let err = run_pnpm(&["--global-bin-dir", "/tmp", "install", "foo"], None).unwrap_err();
        assert!(err.to_string().contains("blocked flag: --global-bin-dir"));
    }

    #[test]
    fn test_pnpm_still_blocks_global_dir() {
        let err = run_pnpm(&["--global-dir", "/tmp"], None).unwrap_err();
        assert!(err.to_string().contains("blocked flag: --global-dir"));
    }

    // Normal flags passthrough

    #[test]
    fn test_pnpm_filter_flag_passthrough() {
        let args = run_pnpm(&["--filter", "@acme/admin", "build"], None).unwrap();
        assert_eq!(args, vec!["--filter", "@acme/admin", "build"]);
    }

    #[test]
    fn test_pnpm_recursive_flag_passthrough() {
        let args = run_pnpm(&["--recursive", "install"], None).unwrap();
        assert_eq!(args, vec!["--recursive", "install"]);
    }

    #[test]
    fn test_pnpm_frozen_lockfile_flag_passthrough() {
        let args = run_pnpm(&["--frozen-lockfile", "install"], None).unwrap();
        assert_eq!(args, vec!["--frozen-lockfile", "install"]);
    }

    // --- tsc.lua tests ---

    fn tsc_script() -> &'static str {
        include_str!("default_scripts/tsc.lua")
    }

    fn tsc_worker_context() -> WorkerContext {
        WorkerContext {
            worker_id: "worker-1".into(),
            process_id: "ur-abc12".into(),
            project_key: "myproject".into(),
            slot_path: PathBuf::from("/home/user/.ur/workspace/pool/myproject/0"),
            branch: "feature-branch".into(),
        }
    }

    fn run_tsc(args: &[&str], ctx: Option<&WorkerContext>) -> anyhow::Result<Vec<String>> {
        let mgr = LuaTransformManager::new();
        let string_args: Vec<String> = args.iter().map(|s| (*s).to_string()).collect();
        let result = mgr.run_transform(tsc_script(), "tsc", &string_args, "/workspace", ctx)?;
        Ok(result.args)
    }

    // Allowed: basic invocations

    #[test]
    fn test_tsc_allows_bare() {
        let args = run_tsc(&[], None).unwrap();
        assert!(args.is_empty());
    }

    #[test]
    fn test_tsc_allows_no_emit() {
        let args = run_tsc(&["--noEmit"], None).unwrap();
        assert_eq!(args, vec!["--noEmit"]);
    }

    #[test]
    fn test_tsc_allows_strict() {
        let args = run_tsc(&["--strict"], None).unwrap();
        assert_eq!(args, vec!["--strict"]);
    }

    #[test]
    fn test_tsc_allows_target() {
        let args = run_tsc(&["--target", "ES2020"], None).unwrap();
        assert_eq!(args, vec!["--target", "ES2020"]);
    }

    #[test]
    fn test_tsc_allows_module_and_target() {
        let args = run_tsc(&["--module", "ESNext", "--target", "ES2020"], None).unwrap();
        assert_eq!(args, vec!["--module", "ESNext", "--target", "ES2020"]);
    }

    // --project / -p relative passthrough

    #[test]
    fn test_tsc_project_short_relative_passthrough() {
        let args = run_tsc(&["-p", "tsconfig.build.json"], None).unwrap();
        assert_eq!(args, vec!["-p", "tsconfig.build.json"]);
    }

    #[test]
    fn test_tsc_project_long_relative_passthrough() {
        let args = run_tsc(&["--project", "./pkg/tsconfig.json"], None).unwrap();
        assert_eq!(args, vec!["--project", "./pkg/tsconfig.json"]);
    }

    // --project / -p absolute rewriting

    #[test]
    fn test_tsc_project_absolute_workspace_rewrite() {
        let ctx = tsc_worker_context();
        let args = run_tsc(&["--project", "/workspace"], Some(&ctx)).unwrap();
        assert_eq!(
            args,
            vec!["--project", "/home/user/.ur/workspace/pool/myproject/0"]
        );
    }

    #[test]
    fn test_tsc_project_short_absolute_workspace_rewrite() {
        let ctx = tsc_worker_context();
        let args = run_tsc(&["-p", "/workspace"], Some(&ctx)).unwrap();
        assert_eq!(
            args,
            vec!["-p", "/home/user/.ur/workspace/pool/myproject/0"]
        );
    }

    #[test]
    fn test_tsc_project_absolute_project_key_rewrite() {
        let ctx = tsc_worker_context();
        let args = run_tsc(&["--project", "/some/path/myproject"], Some(&ctx)).unwrap();
        assert_eq!(
            args,
            vec!["--project", "/home/user/.ur/workspace/pool/myproject/0"]
        );
    }

    #[test]
    fn test_tsc_project_wrong_absolute_blocked() {
        let ctx = tsc_worker_context();
        let err = run_tsc(&["--project", "/other/path"], Some(&ctx)).unwrap_err();
        assert!(
            err.to_string()
                .contains("does not match project key or 'workspace'")
        );
    }

    #[test]
    fn test_tsc_project_no_context_absolute_blocked() {
        let err = run_tsc(&["--project", "/workspace"], None).unwrap_err();
        assert!(
            err.to_string()
                .contains("absolute path requires worker context")
        );
    }

    #[test]
    fn test_tsc_project_equals_form_rejected() {
        let err = run_tsc(&["--project=/workspace"], None).unwrap_err();
        assert!(
            err.to_string()
                .contains("blocked flag: --project=<path> (use --project <path> instead)")
        );
    }

    #[test]
    fn test_tsc_p_equals_form_rejected() {
        let err = run_tsc(&["-p=/workspace"], None).unwrap_err();
        assert!(
            err.to_string()
                .contains("blocked flag: -p=<path> (use -p <path> instead)")
        );
    }

    // Blocked output flags (spaced form — flag name only is an error too)

    #[test]
    fn test_tsc_blocks_out_file() {
        let err = run_tsc(&["--outFile", "dist/bundle.js"], None).unwrap_err();
        assert!(err.to_string().contains("blocked flag: --outFile"));
    }

    #[test]
    fn test_tsc_blocks_out_file_equals() {
        let err = run_tsc(&["--outFile=dist/bundle.js"], None).unwrap_err();
        assert!(err.to_string().contains("blocked flag: --outFile"));
    }

    #[test]
    fn test_tsc_blocks_out_dir() {
        let err = run_tsc(&["--outDir", "dist"], None).unwrap_err();
        assert!(err.to_string().contains("blocked flag: --outDir"));
    }

    #[test]
    fn test_tsc_blocks_out_dir_equals() {
        let err = run_tsc(&["--outDir=dist"], None).unwrap_err();
        assert!(err.to_string().contains("blocked flag: --outDir"));
    }

    #[test]
    fn test_tsc_blocks_declaration_dir() {
        let err = run_tsc(&["--declarationDir", "types"], None).unwrap_err();
        assert!(err.to_string().contains("blocked flag: --declarationDir"));
    }

    #[test]
    fn test_tsc_blocks_declaration_dir_equals() {
        let err = run_tsc(&["--declarationDir=types"], None).unwrap_err();
        assert!(err.to_string().contains("blocked flag: --declarationDir"));
    }

    #[test]
    fn test_tsc_blocks_ts_build_info_file() {
        let err = run_tsc(&["--tsBuildInfoFile", ".tsbuildinfo"], None).unwrap_err();
        assert!(err.to_string().contains("blocked flag: --tsBuildInfoFile"));
    }

    #[test]
    fn test_tsc_blocks_ts_build_info_file_equals() {
        let err = run_tsc(&["--tsBuildInfoFile=.tsbuildinfo"], None).unwrap_err();
        assert!(err.to_string().contains("blocked flag: --tsBuildInfoFile"));
    }

    #[test]
    fn test_tsc_blocks_base_url() {
        let err = run_tsc(&["--baseUrl", "."], None).unwrap_err();
        assert!(err.to_string().contains("blocked flag: --baseUrl"));
    }

    #[test]
    fn test_tsc_blocks_base_url_equals() {
        let err = run_tsc(&["--baseUrl=."], None).unwrap_err();
        assert!(err.to_string().contains("blocked flag: --baseUrl"));
    }

    #[test]
    fn test_tsc_blocks_root_dir() {
        let err = run_tsc(&["--rootDir", "src"], None).unwrap_err();
        assert!(err.to_string().contains("blocked flag: --rootDir"));
    }

    #[test]
    fn test_tsc_blocks_root_dir_equals() {
        let err = run_tsc(&["--rootDir=src"], None).unwrap_err();
        assert!(err.to_string().contains("blocked flag: --rootDir"));
    }

    #[test]
    fn test_tsc_blocks_root_dirs() {
        let err = run_tsc(&["--rootDirs", "src,lib"], None).unwrap_err();
        assert!(err.to_string().contains("blocked flag: --rootDirs"));
    }

    #[test]
    fn test_tsc_blocks_root_dirs_equals() {
        let err = run_tsc(&["--rootDirs=src,lib"], None).unwrap_err();
        assert!(err.to_string().contains("blocked flag: --rootDirs"));
    }

    // Blocked watch flags

    #[test]
    fn test_tsc_blocks_watch() {
        let err = run_tsc(&["--watch"], None).unwrap_err();
        assert!(err.to_string().contains("blocked flag: --watch"));
    }

    #[test]
    fn test_tsc_blocks_watch_short() {
        let err = run_tsc(&["-w"], None).unwrap_err();
        assert!(err.to_string().contains("blocked flag: -w"));
    }

    // Blocked build flags

    #[test]
    fn test_tsc_blocks_build() {
        let err = run_tsc(&["--build"], None).unwrap_err();
        assert!(err.to_string().contains("blocked flag: --build"));
    }

    #[test]
    fn test_tsc_blocks_build_short() {
        let err = run_tsc(&["-b"], None).unwrap_err();
        assert!(err.to_string().contains("blocked flag: -b"));
    }

    // Passthrough flags

    #[test]
    fn test_tsc_passthrough_declaration() {
        let args = run_tsc(&["--declaration"], None).unwrap();
        assert_eq!(args, vec!["--declaration"]);
    }

    #[test]
    fn test_tsc_passthrough_source_map() {
        let args = run_tsc(&["--sourceMap"], None).unwrap();
        assert_eq!(args, vec!["--sourceMap"]);
    }

    #[test]
    fn test_tsc_passthrough_list_files() {
        let args = run_tsc(&["--listFiles"], None).unwrap();
        assert_eq!(args, vec!["--listFiles"]);
    }

    #[test]
    fn test_tsc_passthrough_init() {
        let args = run_tsc(&["--init"], None).unwrap();
        assert_eq!(args, vec!["--init"]);
    }
}
