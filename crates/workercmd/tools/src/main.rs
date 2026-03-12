use std::io::Write;

use clap::{Parser, Subcommand};
use tonic::transport::Endpoint;

use ur_rpc::proto::core::command_output::Payload;
use ur_rpc::proto::hostexec::HostExecRequest;
use ur_rpc::proto::hostexec::host_exec_service_client::HostExecServiceClient;

mod init_git_hooks;
mod init_skills;
mod logging;

#[derive(Parser)]
#[command(name = "ur-tools", about = "Ur worker toolkit")]
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
    /// Run all container initialization (skills, git hooks)
    Init,
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    match cli.command {
        Commands::HostExec { command, args } => {
            std::process::exit(run_host_exec(&command, args).await);
        }
        Commands::Init => {
            logging::init();
            let skills_manager = init_skills::InitSkillsManager::from_env();
            let exit_code = skills_manager.run().await;
            if exit_code != 0 {
                std::process::exit(exit_code);
            }

            let git_hooks_manager = init_git_hooks::InitGitHooksManager;
            if let Err(e) = git_hooks_manager.run().await {
                eprintln!("init git hooks failed: {e}");
                std::process::exit(1);
            }

            std::process::exit(0);
        }
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

    // Inject agent ID metadata header if available
    if let Ok(agent_id) = std::env::var(ur_config::UR_AGENT_ID_ENV)
        && let Ok(val) = agent_id.parse::<tonic::metadata::MetadataValue<tonic::metadata::Ascii>>()
    {
        request
            .metadata_mut()
            .insert(ur_config::AGENT_ID_HEADER, val);
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
