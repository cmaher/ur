use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "ur", about = "Coding LLM coordination framework")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Launch the TUI dashboard
    Tui,
    /// Manage processes
    Process {
        #[command(subcommand)]
        command: ProcessCommands,
    },
    /// Manage tickets
    Ticket {
        #[command(subcommand)]
        command: TicketCommands,
    },
}

#[derive(Subcommand)]
enum ProcessCommands {
    /// Launch a new agent process
    Launch { ticket_id: String },
    /// Show process status
    Status { process_id: Option<String> },
    /// Attach to a running process
    Attach { process_id: String },
}

#[derive(Subcommand)]
enum TicketCommands {
    /// Create a new ticket
    Create {
        title: String,
        #[arg(long)]
        parent: Option<String>,
    },
    /// List tickets
    Ls,
    /// Show ticket details
    Show { ticket_id: String },
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    match cli.command {
        Commands::Tui => println!("Launching TUI..."),
        Commands::Process { command } => match command {
            ProcessCommands::Launch { ticket_id } => {
                println!("Launching process for ticket {ticket_id}...");
            }
            ProcessCommands::Status { process_id } => {
                println!("Status: {process_id:?}");
            }
            ProcessCommands::Attach { process_id } => {
                println!("Attaching to {process_id}...");
            }
        },
        Commands::Ticket { command } => match command {
            TicketCommands::Create { title, parent } => {
                println!("Creating ticket: {title} (parent: {parent:?})");
            }
            TicketCommands::Ls => println!("Listing tickets..."),
            TicketCommands::Show { ticket_id } => {
                println!("Showing ticket {ticket_id}...");
            }
        },
    }
}
