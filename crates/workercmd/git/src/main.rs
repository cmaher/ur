use std::io::Write;

use tonic::transport::Endpoint;
use ur_rpc::proto::core::command_output::Payload;
use ur_rpc::proto::git::GitExecRequest;
use ur_rpc::proto::git::git_service_client::GitServiceClient;

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();

    let grpc_host = std::env::var(ur_config::UR_GRPC_HOST_ENV).expect("UR_GRPC_HOST must be set");
    let grpc_port = std::env::var(ur_config::UR_GRPC_PORT_ENV).expect("UR_GRPC_PORT must be set");
    let addr = format!("http://{grpc_host}:{grpc_port}");

    let channel = match Endpoint::try_from(addr).unwrap().connect().await {
        Ok(ch) => ch,
        Err(e) => {
            eprintln!("git: failed to connect to ur daemon: {e}");
            std::process::exit(1);
        }
    };

    let mut client = GitServiceClient::new(channel);

    let response = match client.exec(GitExecRequest { args }).await {
        Ok(resp) => resp,
        Err(status) => {
            eprintln!("git: {}", status.message());
            std::process::exit(1);
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

    std::process::exit(exit_code);
}
