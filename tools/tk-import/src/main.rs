use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use tonic::transport::Channel;
use ur_rpc::proto::ticket::ticket_service_client::TicketServiceClient;
use ur_rpc::proto::ticket::*;

#[derive(Debug, Deserialize)]
struct TkFrontmatter {
    id: String,
    status: String,
    #[serde(default)]
    deps: Vec<String>,
    #[serde(default)]
    links: Vec<String>,
    created: String,
    #[serde(rename = "type")]
    type_: String,
    priority: i64,
    #[serde(default)]
    assignee: Option<String>,
    #[serde(default)]
    parent: Option<String>,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    branch: Option<String>,
    #[serde(default, rename = "external-ref")]
    external_ref: Option<String>,
}

struct ParsedTicket {
    front: TkFrontmatter,
    title: String,
    body: String,
}

fn is_duplicate_err(e: &tonic::Status) -> bool {
    e.code() == tonic::Code::AlreadyExists || e.message().contains("UNIQUE constraint failed")
}

fn parse_ticket_file(path: &Path) -> Result<ParsedTicket, String> {
    let content =
        std::fs::read_to_string(path).map_err(|e| format!("read {}: {e}", path.display()))?;

    let content = content.trim_start();
    if !content.starts_with("---") {
        return Err(format!("{}: no YAML frontmatter", path.display()));
    }
    let after_first = &content[3..];
    let end = after_first
        .find("\n---")
        .ok_or_else(|| format!("{}: unterminated frontmatter", path.display()))?;
    let yaml_str = &after_first[..end];
    let markdown = after_first[end + 4..].trim();

    let front: TkFrontmatter = serde_yaml::from_str(yaml_str)
        .map_err(|e| format!("{}: YAML parse error: {e}", path.display()))?;

    let title = markdown
        .lines()
        .find(|l| l.starts_with("# "))
        .map(|l| l[2..].trim().to_string())
        .unwrap_or_default();

    let body = if let Some(pos) = markdown.find('\n') {
        markdown[pos + 1..].trim().to_string()
    } else {
        String::new()
    };

    Ok(ParsedTicket { front, title, body })
}

async fn run_migration(
    tickets_dir: &Path,
    client: &mut TicketServiceClient<Channel>,
    dry_run: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let pattern = tickets_dir.join("*.md");
    let paths: Vec<PathBuf> = glob::glob(pattern.to_str().unwrap())?
        .filter_map(|r| r.ok())
        .collect();

    println!("Found {} ticket files", paths.len());

    let mut tickets: Vec<ParsedTicket> = Vec::new();
    let mut errors: Vec<String> = Vec::new();

    for path in &paths {
        match parse_ticket_file(path) {
            Ok(t) => tickets.push(t),
            Err(e) => errors.push(e),
        }
    }

    if !errors.is_empty() {
        eprintln!("\nParse errors ({}):", errors.len());
        for e in &errors {
            eprintln!("  {e}");
        }
    }

    println!("Parsed {} tickets successfully", tickets.len());

    let known_ids: HashSet<&str> = tickets.iter().map(|t| t.front.id.as_str()).collect();

    // Topological sort: parents before children
    let id_to_idx: HashMap<&str, usize> = tickets
        .iter()
        .enumerate()
        .map(|(i, t)| (t.front.id.as_str(), i))
        .collect();

    let mut insert_order: Vec<usize> = Vec::new();
    let mut visited = vec![false; tickets.len()];

    fn visit(
        idx: usize,
        tickets: &[ParsedTicket],
        id_to_idx: &HashMap<&str, usize>,
        visited: &mut Vec<bool>,
        order: &mut Vec<usize>,
    ) {
        if visited[idx] {
            return;
        }
        visited[idx] = true;
        if let Some(ref parent) = tickets[idx].front.parent
            && let Some(&pidx) = id_to_idx.get(parent.as_str())
        {
            visit(pidx, tickets, id_to_idx, visited, order);
        }
        order.push(idx);
    }

    for i in 0..tickets.len() {
        visit(i, &tickets, &id_to_idx, &mut visited, &mut insert_order);
    }

    if dry_run {
        println!("\n[DRY RUN] Would create {} tickets", insert_order.len());
        for &idx in &insert_order {
            let t = &tickets[idx];
            let f = &t.front;
            let type_ = match f.type_.as_str() {
                "epic" => "epic",
                _ => "task",
            };
            println!("  {} [{}] {} - {}", f.id, type_, f.status, t.title);
        }
        return Ok(());
    }

    let mut created = 0u32;
    let mut updated = 0u32;
    let mut edge_count = 0u32;
    let mut meta_count = 0u32;
    let mut skipped_parents = 0u32;

    // Phase 1: Create all tickets
    for &idx in &insert_order {
        let t = &tickets[idx];
        let f = &t.front;

        let status = match f.status.as_str() {
            "in_progress" => "open".to_string(),
            other => other.to_string(),
        };

        let type_ = match f.type_.as_str() {
            "epic" => "epic",
            _ => "task",
        };

        let parent_id = f.parent.as_deref().filter(|p| known_ids.contains(p));
        if f.parent.is_some() && parent_id.is_none() {
            skipped_parents += 1;
            eprintln!(
                "  warn: {} parent {:?} not found, setting to NULL",
                f.id,
                f.parent.as_deref().unwrap()
            );
        }

        match client
            .create_ticket(CreateTicketRequest {
                project: String::new(),
                ticket_type: type_.to_string(),
                status: status.clone(),
                priority: f.priority,
                parent_id: parent_id.map(|s| s.to_string()),
                title: t.title.clone(),
                body: t.body.clone(),
                id: Some(f.id.clone()),
                created_at: Some(f.created.clone()),
            })
            .await
        {
            Ok(_) => created += 1,
            Err(e) if is_duplicate_err(&e) => {
                // Upsert: update existing ticket to match source
                client
                    .update_ticket(UpdateTicketRequest {
                        id: f.id.clone(),
                        status: Some(status),
                        ticket_type: Some(type_.to_string()),
                        priority: Some(f.priority),
                        title: Some(t.title.clone()),
                        body: Some(t.body.clone()),
                        force: true,
                    })
                    .await
                    .map_err(|e| format!("update_ticket {}: {e}", f.id))?;
                updated += 1;
            }
            Err(e) => return Err(format!("create_ticket {}: {e}", f.id).into()),
        }
        if created.is_multiple_of(50) {
            println!("  created {created}/{} tickets...", insert_order.len());
        }
    }

    println!("Created {created} tickets");

    // Phase 2: Add edges (deps = blocks, links = relates_to)
    for t in &tickets {
        let f = &t.front;

        for dep in &f.deps {
            if !known_ids.contains(dep.as_str()) {
                eprintln!("  warn: {} dep {dep} not found, skipping edge", f.id);
                continue;
            }
            match client
                .add_block(AddBlockRequest {
                    blocker_id: dep.clone(),
                    blocked_id: f.id.clone(),
                })
                .await
            {
                Ok(_) => edge_count += 1,
                Err(e) if is_duplicate_err(&e) => {}
                Err(e) => return Err(format!("add_block {} -> {}: {e}", dep, f.id).into()),
            }
        }

        for link in &f.links {
            if !known_ids.contains(link.as_str()) {
                continue;
            }
            // Only insert once: smaller id as left
            if f.id >= *link {
                continue;
            }
            match client
                .add_link(AddLinkRequest {
                    left_id: f.id.clone(),
                    right_id: link.clone(),
                })
                .await
            {
                Ok(_) => edge_count += 1,
                Err(e) if is_duplicate_err(&e) => {}
                Err(e) => return Err(format!("add_link {} <-> {}: {e}", f.id, link).into()),
            }
        }
    }

    // Phase 3: Add metadata (tags, assignee, branch, external-ref)
    for t in &tickets {
        let f = &t.front;

        if !f.tags.is_empty() {
            client
                .set_meta(SetMetaRequest {
                    ticket_id: f.id.clone(),
                    key: "tags".to_string(),
                    value: f.tags.join(","),
                })
                .await
                .map_err(|e| format!("set_meta tags on {}: {e}", f.id))?;
            meta_count += 1;
        }

        if let Some(ref assignee) = f.assignee {
            client
                .set_meta(SetMetaRequest {
                    ticket_id: f.id.clone(),
                    key: "assignee".to_string(),
                    value: assignee.clone(),
                })
                .await
                .map_err(|e| format!("set_meta assignee on {}: {e}", f.id))?;
            meta_count += 1;
        }

        if let Some(ref branch) = f.branch {
            client
                .set_meta(SetMetaRequest {
                    ticket_id: f.id.clone(),
                    key: "branch".to_string(),
                    value: branch.clone(),
                })
                .await
                .map_err(|e| format!("set_meta branch on {}: {e}", f.id))?;
            meta_count += 1;
        }

        if let Some(ref ext_ref) = f.external_ref {
            client
                .set_meta(SetMetaRequest {
                    ticket_id: f.id.clone(),
                    key: "external-ref".to_string(),
                    value: ext_ref.clone(),
                })
                .await
                .map_err(|e| format!("set_meta external-ref on {}: {e}", f.id))?;
            meta_count += 1;
        }
    }

    println!("\nMigration complete:");
    println!("  Tickets created:     {created}");
    if updated > 0 {
        println!("  Tickets updated:     {updated} (already existed)");
    }
    println!("  Edges created:       {edge_count}");
    println!("  Meta entries:        {meta_count}");
    if skipped_parents > 0 {
        println!("  Parent refs missing: {skipped_parents}");
    }

    Ok(())
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().collect();

    let mut tickets_dir = PathBuf::from(".tickets");
    let mut server_addr = "http://127.0.0.1:42069".to_string();
    let mut dry_run = false;

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--tickets" | "-t" => {
                i += 1;
                tickets_dir = PathBuf::from(&args[i]);
            }
            "--server" | "-s" => {
                i += 1;
                server_addr = args[i].clone();
            }
            "--dry-run" | "-n" => {
                dry_run = true;
            }
            "--help" | "-h" => {
                println!("tk-import: migrate tk tickets to ur ticket database via gRPC");
                println!();
                println!("Usage: tk-import [OPTIONS]");
                println!();
                println!("Options:");
                println!("  -t, --tickets <DIR>    Tickets directory (default: .tickets)");
                println!(
                    "  -s, --server <ADDR>    Server address (default: http://127.0.0.1:42069)"
                );
                println!("  -n, --dry-run          Parse and show what would be imported");
                println!("  -h, --help             Show this help");
                return;
            }
            other => {
                eprintln!("Unknown argument: {other}");
                std::process::exit(1);
            }
        }
        i += 1;
    }

    println!("tk -> ur ticket migration (via gRPC)");
    println!("  Source:  {}", tickets_dir.display());
    println!("  Server:  {server_addr}");
    if dry_run {
        println!("  Mode:    DRY RUN");
    }
    println!();

    if !dry_run {
        let channel = Channel::from_shared(server_addr)
            .expect("invalid server address")
            .connect()
            .await;

        let channel = match channel {
            Ok(c) => c,
            Err(e) => {
                eprintln!("Failed to connect to server: {e}");
                eprintln!("Is ur-server running? Start it with: ur start");
                std::process::exit(1);
            }
        };

        let mut client = TicketServiceClient::new(channel);
        if let Err(e) = run_migration(&tickets_dir, &mut client, false).await {
            eprintln!("Migration failed: {e}");
            std::process::exit(1);
        }
    } else {
        // Dry run doesn't need a server connection
        let dummy_channel = Channel::from_static("http://[::1]:1").connect_lazy();
        let mut client = TicketServiceClient::new(dummy_channel);
        if let Err(e) = run_migration(&tickets_dir, &mut client, true).await {
            eprintln!("Migration failed: {e}");
            std::process::exit(1);
        }
    }
}
