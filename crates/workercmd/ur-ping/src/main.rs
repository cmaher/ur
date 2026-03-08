use tonic::transport::Endpoint;
use ur_rpc::proto::core::PingRequest;
use ur_rpc::proto::core::core_service_client::CoreServiceClient;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let grpc_host = std::env::var(ur_config::UR_GRPC_HOST_ENV).expect("UR_GRPC_HOST must be set");
    let grpc_port = std::env::var(ur_config::UR_GRPC_PORT_ENV).expect("UR_GRPC_PORT must be set");
    let addr = format!("http://{grpc_host}:{grpc_port}");

    let channel = Endpoint::try_from(addr)?.connect().await?;

    let mut client = CoreServiceClient::new(channel);
    let resp = client.ping(PingRequest {}).await?;
    println!("{}", resp.into_inner().message);

    Ok(())
}
