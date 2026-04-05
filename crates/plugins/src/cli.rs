use anyhow::Result;
use compose::ComposeFile;

/// Plugin interface for CLI-side extensions.
///
/// Implement this trait to add CLI commands, modify compose configuration,
/// or hook into the CLI lifecycle.
pub trait CliPlugin: Send + Sync {
    /// Unique name identifying this plugin.
    fn name(&self) -> &str;

    /// Called once at startup with the plugin's TOML configuration table.
    /// Default: no-op.
    fn configure(&mut self, _config: &toml::Table) -> Result<()> {
        Ok(())
    }

    /// Modify the Docker Compose file before it is written/applied.
    /// Default: no-op.
    fn modify_compose(&self, _compose: &mut ComposeFile) -> Result<()> {
        Ok(())
    }

    /// Register additional clap subcommands.
    /// Default: no-op.
    fn register_cli(&self, _app: clap::Command) -> clap::Command {
        _app
    }

    /// Handle an invocation of a plugin-registered subcommand.
    /// Returns Ok(true) if the command was handled, Ok(false) otherwise.
    /// Default: not handled.
    fn handle_cli(&self, _matches: &clap::ArgMatches) -> Result<bool> {
        Ok(false)
    }
}

/// Registry holding CLI plugins and providing batch-apply methods.
pub struct CliRegistry {
    plugins: Vec<Box<dyn CliPlugin>>,
}

impl CliRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self {
            plugins: Vec::new(),
        }
    }

    /// Register a plugin.
    pub fn register(&mut self, plugin: Box<dyn CliPlugin>) {
        self.plugins.push(plugin);
    }

    /// Apply `modify_compose` from every registered plugin, in order.
    pub fn apply_compose(&self, compose: &mut ComposeFile) -> Result<()> {
        for plugin in &self.plugins {
            plugin.modify_compose(compose)?;
        }
        Ok(())
    }

    /// Let each plugin register its CLI subcommands.
    pub fn apply_register_cli(&self, mut app: clap::Command) -> clap::Command {
        for plugin in &self.plugins {
            app = plugin.register_cli(app);
        }
        app
    }

    /// Route a CLI invocation through plugins until one handles it.
    /// Returns Ok(true) if any plugin handled the command.
    pub fn apply_handle_cli(&self, matches: &clap::ArgMatches) -> Result<bool> {
        for plugin in &self.plugins {
            if plugin.handle_cli(matches)? {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Iterate over registered plugins.
    pub fn plugins(&self) -> &[Box<dyn CliPlugin>] {
        &self.plugins
    }
}

impl Default for CliRegistry {
    fn default() -> Self {
        Self::new()
    }
}
