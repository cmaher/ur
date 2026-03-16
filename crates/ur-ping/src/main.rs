use tonic::transport::Endpoint;
use ur_rpc::proto::core::PingRequest;
use ur_rpc::proto::core::core_service_client::CoreServiceClient;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let server_addr =
        std::env::var(ur_config::UR_SERVER_ADDR_ENV).expect("UR_SERVER_ADDR must be set");
    let addr = format!("http://{server_addr}");

    let channel = Endpoint::try_from(addr)?.connect().await?;

    let mut client = CoreServiceClient::new(channel);

    let mut request = tonic::Request::new(PingRequest {});

    // Inject worker ID and secret metadata headers if available
    if let Ok(worker_id) = std::env::var(ur_config::UR_AGENT_ID_ENV)
        && let Ok(val) = worker_id.parse::<tonic::metadata::MetadataValue<tonic::metadata::Ascii>>()
    {
        request
            .metadata_mut()
            .insert(ur_config::AGENT_ID_HEADER, val);
    }
    if let Ok(worker_secret) = std::env::var(ur_config::UR_AGENT_SECRET_ENV)
        && let Ok(val) =
            worker_secret.parse::<tonic::metadata::MetadataValue<tonic::metadata::Ascii>>()
    {
        request
            .metadata_mut()
            .insert(ur_config::AGENT_SECRET_HEADER, val);
    }

    let resp = client.ping(request).await?;
    println!("{}", resp.into_inner().message);

    Ok(())
}
