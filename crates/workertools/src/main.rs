use std::io::Write;

use clap::{Parser, Subcommand};
use tonic::transport::Endpoint;

use ur_rpc::proto::core::command_output::Payload;
use ur_rpc::proto::hostexec::HostExecRequest;
use ur_rpc::proto::hostexec::host_exec_service_client::HostExecServiceClient;
use ur_rpc::proto::rag::rag_service_client::RagServiceClient;
use ur_rpc::proto::rag::{Language, RagSearchRequest};

#[derive(Parser)]
#[command(name = "workertools", about = "Ur worker toolkit")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Execute a command on the host via ur-server
    HostExec {
        /// The command to execute
        command: String,
        /// Arguments to the command
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// RAG operations
    Rag {
        #[command(subcommand)]
        command: RagCommands,
    },
}

#[derive(Subcommand)]
enum RagCommands {
    /// Search indexed documentation via RAG
    Search {
        /// The search query
        query: String,
        /// Language to search (default: rust)
        #[arg(long, default_value = "rust")]
        language: String,
        /// Number of results to return (default: 5)
        #[arg(long, default_value_t = 5)]
        top_k: u32,
    },
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    match cli.command {
        Commands::HostExec { command, args } => {
            std::process::exit(run_host_exec(&command, args).await);
        }
        Commands::Rag { command } => match command {
            RagCommands::Search {
                query,
                language,
                top_k,
            } => {
                std::process::exit(run_rag_search(&query, &language, top_k).await);
            }
        },
    }
}

async fn run_host_exec(command: &str, args: Vec<String>) -> i32 {
    let server_addr =
        std::env::var(ur_config::UR_SERVER_ADDR_ENV).expect("UR_SERVER_ADDR must be set");
    let addr = format!("http://{server_addr}");

    let channel = match Endpoint::try_from(addr).unwrap().connect().await {
        Ok(ch) => ch,
        Err(e) => {
            eprintln!("{command}: failed to connect to ur server: {e}");
            return 1;
        }
    };

    let mut client = HostExecServiceClient::new(channel);

    let working_dir = std::env::current_dir()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| "/workspace".into());

    let mut request = tonic::Request::new(HostExecRequest {
        command: command.into(),
        args,
        working_dir,
    });

    // Inject agent ID and secret metadata headers if available
    if let Ok(agent_id) = std::env::var(ur_config::UR_AGENT_ID_ENV)
        && let Ok(val) = agent_id.parse::<tonic::metadata::MetadataValue<tonic::metadata::Ascii>>()
    {
        request
            .metadata_mut()
            .insert(ur_config::AGENT_ID_HEADER, val);
    }
    if let Ok(agent_secret) = std::env::var(ur_config::UR_AGENT_SECRET_ENV)
        && let Ok(val) =
            agent_secret.parse::<tonic::metadata::MetadataValue<tonic::metadata::Ascii>>()
    {
        request
            .metadata_mut()
            .insert(ur_config::AGENT_SECRET_HEADER, val);
    }

    let response = match client.exec(request).await {
        Ok(resp) => resp,
        Err(status) => {
            eprintln!("{command}: {}", status.message());
            return 1;
        }
    };

    let mut stream = response.into_inner();
    let mut exit_code = 1;

    while let Ok(Some(msg)) = stream.message().await {
        let Some(payload) = msg.payload else {
            continue;
        };
        match payload {
            Payload::Stdout(data) => {
                let _ = std::io::stdout().write_all(&data);
                let _ = std::io::stdout().flush();
            }
            Payload::Stderr(data) => {
                let _ = std::io::stderr().write_all(&data);
                let _ = std::io::stderr().flush();
            }
            Payload::ExitCode(code) => exit_code = code,
        }
    }

    exit_code
}

fn parse_language(s: &str) -> Option<Language> {
    match s.to_lowercase().as_str() {
        "rust" => Some(Language::Rust),
        _ => None,
    }
}

async fn run_rag_search(query: &str, language: &str, top_k: u32) -> i32 {
    let lang = match parse_language(language) {
        Some(l) => l,
        None => {
            eprintln!("rag search: unsupported language: {language}");
            return 1;
        }
    };

    let server_addr =
        std::env::var(ur_config::UR_SERVER_ADDR_ENV).expect("UR_SERVER_ADDR must be set");
    let addr = format!("http://{server_addr}");

    let channel = match Endpoint::try_from(addr).unwrap().connect().await {
        Ok(ch) => ch,
        Err(e) => {
            eprintln!("rag search: failed to connect to ur server: {e}");
            return 1;
        }
    };

    let mut client = RagServiceClient::new(channel);

    let mut request = tonic::Request::new(RagSearchRequest {
        query: query.to_owned(),
        language: lang.into(),
        top_k: Some(top_k),
    });

    // Inject agent ID and secret metadata headers if available
    if let Ok(agent_id) = std::env::var(ur_config::UR_AGENT_ID_ENV)
        && let Ok(val) = agent_id.parse::<tonic::metadata::MetadataValue<tonic::metadata::Ascii>>()
    {
        request
            .metadata_mut()
            .insert(ur_config::AGENT_ID_HEADER, val);
    }
    if let Ok(agent_secret) = std::env::var(ur_config::UR_AGENT_SECRET_ENV)
        && let Ok(val) =
            agent_secret.parse::<tonic::metadata::MetadataValue<tonic::metadata::Ascii>>()
    {
        request
            .metadata_mut()
            .insert(ur_config::AGENT_SECRET_HEADER, val);
    }

    let resp = match client.rag_search(request).await {
        Ok(resp) => resp,
        Err(status) => {
            eprintln!("rag search: {}", status.message());
            return 1;
        }
    };

    let results = resp.into_inner().results;
    if results.is_empty() {
        println!("No results found.");
        return 0;
    }

    for (i, result) in results.iter().enumerate() {
        println!("--- Result {} (score: {:.2}) ---", i + 1, result.score);
        println!("Source: {}", result.source_file);
        println!();
        println!("{}", result.text);
        println!();
    }

    0
}
