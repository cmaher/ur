use std::io::Write;

use hyper_util::rt::TokioIo;
use tokio::net::UnixStream;
use tonic::transport::{Endpoint, Uri};
use tower::service_fn;
use ur_rpc::proto::core::command_output::Payload;
use ur_rpc::proto::git::GitExecRequest;
use ur_rpc::proto::git::git_service_client::GitServiceClient;

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();

    // Intercept --help and --version to identify this as the ur git proxy
    if args.iter().any(|a| a == "--help" || a == "-h") {
        eprintln!("ur git proxy — transparently forwards git commands to urd's GitService");
        eprintln!("Usage: git <args...>");
        eprintln!();
        eprintln!("All arguments are sent to the host daemon via gRPC over UDS.");
        eprintln!("Set $UR_SOCKET to override the default socket path.");
        std::process::exit(0);
    }
    if args.iter().any(|a| a == "--version") {
        eprintln!("ur git proxy {}", env!("CARGO_PKG_VERSION"));
        std::process::exit(0);
    }

    let socket_path = std::env::var("UR_SOCKET").unwrap_or_else(|_| "/var/run/ur/ur.sock".into());

    let path = socket_path.clone();
    let channel = match Endpoint::try_from("http://[::]:50051")
        .unwrap()
        .connect_with_connector(service_fn(move |_: Uri| {
            let path = path.clone();
            async move {
                let stream = UnixStream::connect(path).await?;
                Ok::<_, std::io::Error>(TokioIo::new(stream))
            }
        }))
        .await
    {
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
