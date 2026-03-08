use tonic::transport::Endpoint;
use ur_rpc::proto::core::PingRequest;
use ur_rpc::proto::core::core_service_client::CoreServiceClient;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let urd_addr = std::env::var(ur_config::URD_ADDR_ENV).expect("URD_ADDR must be set");
    let addr = format!("http://{urd_addr}");

    let channel = Endpoint::try_from(addr)?.connect().await?;

    let mut client = CoreServiceClient::new(channel);
    let resp = client.ping(PingRequest {}).await?;
    println!("{}", resp.into_inner().message);

    Ok(())
}
