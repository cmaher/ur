mod context;
mod create_ticket;
#[allow(dead_code)]
mod keymap;
mod terminal;
mod theme;
mod v2;
mod widgets;

use clap::Parser;

/// Terminal UI for the Ur coordination framework.
#[derive(Parser)]
#[command(name = "urui")]
struct Cli {
    /// Scope the UI to a single project key. If omitted, attempts to derive
    /// the project from the current directory name.
    #[arg(short = 'p', long = "project")]
    project: Option<String>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    v2::run_v2(cli.project).await
}
