use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "agent_tools", about = "Worker CLI for Ur containers")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Ask a blocking question to the human operator
    Ask { question: String },
    /// Proxy git commands to the host
    Git {
        /// Git arguments
        #[arg(trailing_var_arg = true)]
        args: Vec<String>,
    },
    /// Interact with the ticket system
    Ticket {
        #[command(subcommand)]
        command: TicketCommands,
    },
}

#[derive(Subcommand)]
enum TicketCommands {
    /// Read the current ticket spec
    Read,
    /// Append a note to the current ticket
    Note { message: String },
    /// Spawn a child ticket
    Spawn {
        #[arg(long)]
        title: String,
        #[arg(long)]
        description: Option<String>,
    },
    /// Update ticket status
    Status { status: String },
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    match cli.command {
        Commands::Ask { question } => {
            println!("Asking: {question}");
        }
        Commands::Git { args } => {
            println!("Git: {args:?}");
        }
        Commands::Ticket { command } => match command {
            TicketCommands::Read => println!("Reading ticket..."),
            TicketCommands::Note { message } => {
                println!("Adding note: {message}");
            }
            TicketCommands::Spawn { title, description } => {
                println!("Spawning: {title} ({description:?})");
            }
            TicketCommands::Status { status } => {
                println!("Setting status: {status}");
            }
        },
    }
}
