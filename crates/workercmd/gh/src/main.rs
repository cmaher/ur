use std::io::Write;

use tonic::transport::Endpoint;
use ur_rpc::proto::core::command_output::Payload;
use ur_rpc::proto::gh::gh_service_client::GhServiceClient;
use ur_rpc::proto::gh::GhExecRequest;

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();

    if args.first().map(|a| a.as_str()) == Some("--help") {
        eprintln!("ur gh proxy — forwards gh commands to urd via gRPC");
        std::process::exit(0);
    }

    let urd_addr = std::env::var(ur_config::URD_ADDR_ENV).expect("URD_ADDR must be set");
    let addr = format!("http://{urd_addr}");

    let channel = match Endpoint::try_from(addr).unwrap().connect().await {
        Ok(ch) => ch,
        Err(e) => {
            eprintln!("gh: failed to connect to ur daemon: {e}");
            std::process::exit(1);
        }
    };

    let mut client = GhServiceClient::new(channel);

    let response = match client.exec(GhExecRequest { args }).await {
        Ok(resp) => resp,
        Err(status) => {
            eprintln!("gh: {}", status.message());
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
