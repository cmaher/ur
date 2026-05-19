use anyhow::Result;
use clap::Subcommand;

/// Builder subcommands.
#[derive(Debug, Subcommand)]
pub enum BuilderCommands {
    /// Print the builderd environment (port, workspace, config dir)
    Env,
    /// Locate the named command on the builderd host PATH
    Which {
        /// Command to look up (e.g. "git", "npm")
        command: String,
    },
}

pub fn handle(command: BuilderCommands) -> Result<()> {
    match command {
        BuilderCommands::Env => {
            anyhow::bail!("builder env: not yet implemented");
        }
        BuilderCommands::Which { command } => {
            anyhow::bail!("builder which {command}: not yet implemented");
        }
    }
}
