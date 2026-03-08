use tonic::transport::Endpoint;
use ur_rpc::proto::core::PingRequest;
use ur_rpc::proto::core::core_service_client::CoreServiceClient;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let grpc_host = std::env::var("UR_GRPC_HOST").unwrap_or_else(|_| "127.0.0.1".into());
    let grpc_port = std::env::var("UR_GRPC_PORT").unwrap_or_else(|_| "42069".into());
    let addr = format!("http://{grpc_host}:{grpc_port}");

    let channel = Endpoint::try_from(addr)?.connect().await?;

    let mut client = CoreServiceClient::new(channel);
    let resp = client.ping(PingRequest {}).await?;
    println!("{}", resp.into_inner().message);

    Ok(())
}
