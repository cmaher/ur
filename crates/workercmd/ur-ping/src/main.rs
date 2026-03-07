use hyper_util::rt::TokioIo;
use tokio::net::UnixStream;
use tonic::transport::{Endpoint, Uri};
use tower::service_fn;
use ur_rpc::proto::core::core_service_client::CoreServiceClient;
use ur_rpc::proto::core::PingRequest;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let socket_path =
        std::env::var("UR_SOCKET").unwrap_or_else(|_| "/var/run/ur/ur.sock".into());

    let path = socket_path.clone();
    let channel = Endpoint::try_from("http://[::]:50051")?
        .connect_with_connector(service_fn(move |_: Uri| {
            let path = path.clone();
            async move {
                let stream = UnixStream::connect(path).await?;
                Ok::<_, std::io::Error>(TokioIo::new(stream))
            }
        }))
        .await?;

    let mut client = CoreServiceClient::new(channel);
    let resp = client.ping(PingRequest {}).await?;
    println!("{}", resp.into_inner().message);

    Ok(())
}
