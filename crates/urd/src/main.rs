use clap::Parser;

#[derive(Parser)]
#[command(
    name = "urd",
    about = "Ur daemon — coordination server for containerized agents"
)]
struct Cli {
    /// Socket directory for agent UDS connections
    #[arg(long, default_value = "/tmp/ur/sockets")]
    socket_dir: String,
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    println!("Starting urd (socket_dir: {})...", cli.socket_dir);
}
