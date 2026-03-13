use anyhow::{Context, Result};
use tonic::transport::{Channel, Endpoint};
use tracing::{debug, info};
use ur_rpc::proto::ticket::ticket_service_client::TicketServiceClient;
use ur_rpc::proto::ticket::*;

async fn connect_ticket(port: u16) -> Result<TicketServiceClient<Channel>> {
    let addr = format!("http://127.0.0.1:{port}");
    let channel = Endpoint::try_from(addr)?
        .connect()
        .await
        .context("server is not running — run 'ur start' first")?;
    Ok(TicketServiceClient::new(channel))
}

pub async fn create(
    port: u16,
    title: &str,
    ticket_type: &str,
    parent: Option<&str>,
    priority: i64,
    body: &str,
    project: &str,
) -> Result<()> {
    info!(title, ticket_type, parent, priority, "creating ticket");
    let mut client = connect_ticket(port).await?;
    let resp = client
        .create_ticket(CreateTicketRequest {
            project: project.to_owned(),
            ticket_type: ticket_type.to_owned(),
            status: "open".to_owned(),
            priority,
            parent_id: parent.map(|s| s.to_owned()),
            title: title.to_owned(),
            body: body.to_owned(),
        })
        .await
        .context("failed to create ticket")?;
    let id = resp.into_inner().id;
    println!("Created {id}");
    Ok(())
}

pub async fn list(
    port: u16,
    epic: Option<&str>,
    ticket_type: Option<&str>,
    status: Option<&str>,
) -> Result<()> {
    debug!(epic, ticket_type, status, "listing tickets");
    let mut client = connect_ticket(port).await?;
    let resp = client
        .list_tickets(ListTicketsRequest {
            project: None,
            ticket_type: ticket_type.map(|s| s.to_owned()),
            status: status.map(|s| s.to_owned()),
            parent_id: epic.map(|s| s.to_owned()),
            meta_key: None,
            meta_value: None,
        })
        .await
        .context("failed to list tickets")?;
    let tickets = resp.into_inner().tickets;
    if tickets.is_empty() {
        println!("No tickets found.");
        return Ok(());
    }
    // Header
    println!(
        "{:<20} {:<10} {:<14} {:<4} TITLE",
        "ID", "TYPE", "STATUS", "PRI"
    );
    let separator: String = std::iter::repeat_n('-', 72).collect();
    println!("{separator}");
    for t in &tickets {
        println!(
            "{:<20} {:<10} {:<14} {:<4} {}",
            t.id, t.ticket_type, t.status, t.priority, t.title
        );
    }
    println!("\n{} ticket(s)", tickets.len());
    Ok(())
}

pub async fn show(port: u16, ticket_id: &str) -> Result<()> {
    debug!(ticket_id, "showing ticket");
    let mut client = connect_ticket(port).await?;
    let resp = client
        .get_ticket(GetTicketRequest {
            id: ticket_id.to_owned(),
        })
        .await
        .context("failed to get ticket")?;
    let inner = resp.into_inner();
    let t = inner
        .ticket
        .as_ref()
        .context("server returned empty ticket")?;

    println!("ID:       {}", t.id);
    println!("Title:    {}", t.title);
    println!("Type:     {}", t.ticket_type);
    println!("Status:   {}", t.status);
    println!("Priority: {}", t.priority);
    if !t.parent_id.is_empty() {
        println!("Parent:   {}", t.parent_id);
    }
    println!("Created:  {}", t.created_at);
    println!("Updated:  {}", t.updated_at);
    if !t.body.is_empty() {
        println!("\n{}", t.body);
    }
    if !inner.metadata.is_empty() {
        println!("\nMetadata:");
        for m in &inner.metadata {
            println!("  {}: {}", m.key, m.value);
        }
    }
    if !inner.activities.is_empty() {
        println!("\nActivity:");
        for a in &inner.activities {
            println!("  [{}] {}: {}", a.timestamp, a.author, a.message);
        }
    }
    Ok(())
}

pub async fn update(
    port: u16,
    ticket_id: &str,
    status: Option<&str>,
    priority: Option<i64>,
    title: Option<&str>,
    body: Option<&str>,
) -> Result<()> {
    info!(ticket_id, status, priority, title, "updating ticket");
    let mut client = connect_ticket(port).await?;
    client
        .update_ticket(UpdateTicketRequest {
            id: ticket_id.to_owned(),
            status: status.map(|s| s.to_owned()),
            priority,
            title: title.map(|s| s.to_owned()),
            body: body.map(|s| s.to_owned()),
        })
        .await
        .context("failed to update ticket")?;
    println!("Updated {ticket_id}");
    Ok(())
}

pub async fn add_dep(port: u16, ticket_id: &str, blocker_id: &str) -> Result<()> {
    info!(ticket_id, blocker_id, "adding dependency");
    let mut client = connect_ticket(port).await?;
    client
        .add_block(AddBlockRequest {
            blocker_id: blocker_id.to_owned(),
            blocked_id: ticket_id.to_owned(),
        })
        .await
        .context("failed to add dependency")?;
    println!("{blocker_id} now blocks {ticket_id}");
    Ok(())
}

pub async fn remove_dep(port: u16, ticket_id: &str, blocker_id: &str) -> Result<()> {
    info!(ticket_id, blocker_id, "removing dependency");
    let mut client = connect_ticket(port).await?;
    client
        .remove_block(RemoveBlockRequest {
            blocker_id: blocker_id.to_owned(),
            blocked_id: ticket_id.to_owned(),
        })
        .await
        .context("failed to remove dependency")?;
    println!("{blocker_id} no longer blocks {ticket_id}");
    Ok(())
}

pub async fn add_link(port: u16, id1: &str, id2: &str) -> Result<()> {
    info!(id1, id2, "linking tickets");
    let mut client = connect_ticket(port).await?;
    client
        .add_link(AddLinkRequest {
            left_id: id1.to_owned(),
            right_id: id2.to_owned(),
        })
        .await
        .context("failed to link tickets")?;
    println!("Linked {id1} <-> {id2}");
    Ok(())
}

pub async fn remove_link(port: u16, id1: &str, id2: &str) -> Result<()> {
    info!(id1, id2, "unlinking tickets");
    let mut client = connect_ticket(port).await?;
    client
        .remove_link(RemoveLinkRequest {
            left_id: id1.to_owned(),
            right_id: id2.to_owned(),
        })
        .await
        .context("failed to unlink tickets")?;
    println!("Unlinked {id1} <-> {id2}");
    Ok(())
}

pub async fn add_note(port: u16, ticket_id: &str, message: &str) -> Result<()> {
    info!(ticket_id, "adding note");
    let mut client = connect_ticket(port).await?;
    let resp = client
        .add_activity(AddActivityRequest {
            ticket_id: ticket_id.to_owned(),
            author: "cli".to_owned(),
            message: message.to_owned(),
            metadata: Default::default(),
        })
        .await
        .context("failed to add note")?;
    let activity_id = resp.into_inner().activity_id;
    println!("Added note {activity_id} to {ticket_id}");
    Ok(())
}

pub async fn set_meta(port: u16, ticket_id: &str, key: &str, value: &str) -> Result<()> {
    info!(ticket_id, key, value, "setting metadata");
    let mut client = connect_ticket(port).await?;
    client
        .set_meta(SetMetaRequest {
            ticket_id: ticket_id.to_owned(),
            key: key.to_owned(),
            value: value.to_owned(),
        })
        .await
        .context("failed to set metadata")?;
    println!("Set {key}={value} on {ticket_id}");
    Ok(())
}

pub async fn delete_meta(port: u16, ticket_id: &str, key: &str) -> Result<()> {
    info!(ticket_id, key, "deleting metadata");
    let mut client = connect_ticket(port).await?;
    client
        .delete_meta(DeleteMetaRequest {
            ticket_id: ticket_id.to_owned(),
            key: key.to_owned(),
        })
        .await
        .context("failed to delete metadata")?;
    println!("Deleted {key} from {ticket_id}");
    Ok(())
}
