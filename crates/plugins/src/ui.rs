use anyhow::Result;

/// Plugin interface for UI extensions.
///
/// Implement this trait to extend the TUI with additional views,
/// keybindings, or event handling.
pub trait UiPlugin: Send + Sync {
    /// Unique name identifying this plugin.
    fn name(&self) -> &str;

    /// Called once at startup with the plugin's TOML configuration table.
    /// Default: no-op.
    fn configure(&mut self, _config: &toml::Table) -> Result<()> {
        Ok(())
    }
}

/// Registry holding UI plugins and providing batch-apply methods.
pub struct UiRegistry {
    plugins: Vec<Box<dyn UiPlugin>>,
}

impl UiRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self {
            plugins: Vec::new(),
        }
    }

    /// Register a plugin.
    pub fn register(&mut self, plugin: Box<dyn UiPlugin>) {
        self.plugins.push(plugin);
    }

    /// Iterate over registered plugins.
    pub fn plugins(&self) -> &[Box<dyn UiPlugin>] {
        &self.plugins
    }
}

impl Default for UiRegistry {
    fn default() -> Self {
        Self::new()
    }
}
