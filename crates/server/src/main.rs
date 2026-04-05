use clap::Parser;
use plugins::ServerRegistry;

#[derive(Parser)]
#[command(
    name = "ur-server",
    about = "Ur server — coordination server for containerized agents"
)]
struct Cli {}

fn main() -> anyhow::Result<()> {
    let _cli = Cli::parse();
    ur_server::run(ServerRegistry::new())
}
