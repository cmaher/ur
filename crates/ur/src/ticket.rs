use anyhow::{Context, Result};
use ticket_client::TicketArgs;
use tonic::transport::{Channel, Endpoint};
use ur_rpc::proto::ticket::ticket_service_client::TicketServiceClient;

use crate::output::OutputManager;

async fn connect_ticket(port: u16) -> Result<TicketServiceClient<Channel>> {
    let addr = format!("http://127.0.0.1:{port}");
    let channel = Endpoint::try_from(addr)?
        .connect()
        .await
        .context("server is not running — run 'ur server start' first")?;
    Ok(TicketServiceClient::new(channel))
}

pub async fn handle(port: u16, args: TicketArgs, output: &OutputManager) -> Result<()> {
    let mut client = connect_ticket(port).await?;
    let result = ticket_client::execute(args, &mut client).await?;
    if output.is_json() {
        output.print_success(&result);
    } else {
        println!("{}", ticket_client::format_output(&result));
    }
    Ok(())
}
