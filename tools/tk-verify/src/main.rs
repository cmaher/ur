use serde::Deserialize;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

use tonic::transport::Channel;
use ur_rpc::proto::ticket::ticket_service_client::TicketServiceClient;
use ur_rpc::proto::ticket::*;

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
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

struct Mismatch {
    ticket_id: String,
    field: String,
    expected: String,
    actual: String,
}

fn expected_status(tk_status: &str) -> &str {
    match tk_status {
        "in_progress" => "open",
        other => other,
    }
}

fn expected_type(tk_type: &str) -> &str {
    match tk_type {
        "epic" => "epic",
        _ => "task",
    }
}

fn check_meta(
    meta: &[MetadataEntry],
    key: &str,
    expected: Option<&str>,
    ticket_id: &str,
    mismatches: &mut Vec<Mismatch>,
) {
    let actual = meta.iter().find(|m| m.key == key).map(|m| m.value.as_str());
    if actual != expected {
        mismatches.push(Mismatch {
            ticket_id: ticket_id.to_string(),
            field: format!("meta:{key}"),
            expected: expected.unwrap_or("<none>").to_string(),
            actual: actual.unwrap_or("<none>").to_string(),
        });
    }
}

fn check_ticket_fields(
    ticket: &Ticket,
    parsed: &ParsedTicket,
    known_ids: &HashSet<&str>,
    mismatches: &mut Vec<Mismatch>,
) {
    let f = &parsed.front;

    if ticket.title != parsed.title {
        mismatches.push(Mismatch {
            ticket_id: f.id.clone(),
            field: "title".into(),
            expected: parsed.title.clone(),
            actual: ticket.title.clone(),
        });
    }

    if ticket.body != parsed.body {
        mismatches.push(Mismatch {
            ticket_id: f.id.clone(),
            field: "body".into(),
            expected: format!("({} chars)", parsed.body.len()),
            actual: format!("({} chars)", ticket.body.len()),
        });
    }

    let exp_status = expected_status(&f.status);
    if ticket.status != exp_status {
        mismatches.push(Mismatch {
            ticket_id: f.id.clone(),
            field: "status".into(),
            expected: exp_status.to_string(),
            actual: ticket.status.clone(),
        });
    }

    let exp_type = expected_type(&f.type_);
    if ticket.ticket_type != exp_type {
        mismatches.push(Mismatch {
            ticket_id: f.id.clone(),
            field: "ticket_type".into(),
            expected: exp_type.to_string(),
            actual: ticket.ticket_type.clone(),
        });
    }

    if ticket.priority != f.priority {
        mismatches.push(Mismatch {
            ticket_id: f.id.clone(),
            field: "priority".into(),
            expected: f.priority.to_string(),
            actual: ticket.priority.to_string(),
        });
    }

    let exp_parent = f
        .parent
        .as_deref()
        .filter(|p| known_ids.contains(p))
        .unwrap_or("");
    if ticket.parent_id != exp_parent {
        mismatches.push(Mismatch {
            ticket_id: f.id.clone(),
            field: "parent_id".into(),
            expected: if exp_parent.is_empty() {
                "<none>".into()
            } else {
                exp_parent.to_string()
            },
            actual: if ticket.parent_id.is_empty() {
                "<none>".into()
            } else {
                ticket.parent_id.clone()
            },
        });
    }
}

fn check_ticket_metadata(
    meta: &[MetadataEntry],
    front: &TkFrontmatter,
    mismatches: &mut Vec<Mismatch>,
) {
    let exp_tags = if front.tags.is_empty() {
        None
    } else {
        Some(front.tags.join(","))
    };
    check_meta(meta, "tags", exp_tags.as_deref(), &front.id, mismatches);
    check_meta(
        meta,
        "assignee",
        front.assignee.as_deref(),
        &front.id,
        mismatches,
    );
    check_meta(
        meta,
        "branch",
        front.branch.as_deref(),
        &front.id,
        mismatches,
    );
    check_meta(
        meta,
        "external-ref",
        front.external_ref.as_deref(),
        &front.id,
        mismatches,
    );
}

async fn verify_tickets(
    tickets: &[ParsedTicket],
    known_ids: &HashSet<&str>,
    client: &mut TicketServiceClient<Channel>,
) -> Result<(Vec<Mismatch>, Vec<String>, u32), Box<dyn std::error::Error>> {
    let mut mismatches: Vec<Mismatch> = Vec::new();
    let mut missing: Vec<String> = Vec::new();
    let mut verified = 0u32;

    for t in tickets {
        let f = &t.front;

        let resp = match client
            .get_ticket(GetTicketRequest { id: f.id.clone() })
            .await
        {
            Ok(r) => r.into_inner(),
            Err(e) => {
                if e.code() == tonic::Code::NotFound || e.message().contains("not found") {
                    missing.push(f.id.clone());
                } else {
                    return Err(format!("get_ticket {}: {e}", f.id).into());
                }
                continue;
            }
        };

        let ticket = resp.ticket.as_ref().unwrap();
        check_ticket_fields(ticket, t, known_ids, &mut mismatches);
        check_ticket_metadata(&resp.metadata, f, &mut mismatches);

        verified += 1;
    }

    Ok((mismatches, missing, verified))
}

fn print_summary(mismatches: &[Mismatch], missing: &[String], verified: u32, total: usize) {
    println!("Verification complete:");
    println!("  Verified:   {verified}/{total}");

    if !missing.is_empty() {
        println!("  Missing:    {} tickets not in ur", missing.len());
        for id in missing {
            println!("    - {id}");
        }
    }

    if mismatches.is_empty() {
        println!("  Mismatches: 0");
        println!("\nAll tickets match.");
    } else {
        println!("  Mismatches: {}", mismatches.len());
        println!();
        for m in mismatches {
            println!(
                "  {} .{}: expected {:?}, got {:?}",
                m.ticket_id, m.field, m.expected, m.actual
            );
        }
    }
}

async fn run_verify(
    tickets_dir: &Path,
    client: &mut TicketServiceClient<Channel>,
) -> Result<(), Box<dyn std::error::Error>> {
    let pattern = tickets_dir.join("*.md");
    let paths: Vec<PathBuf> = glob::glob(pattern.to_str().unwrap())?
        .filter_map(|r| r.ok())
        .collect();

    println!("Found {} ticket files", paths.len());

    let mut tickets: Vec<ParsedTicket> = Vec::new();
    let mut parse_errors: Vec<String> = Vec::new();

    for path in &paths {
        match parse_ticket_file(path) {
            Ok(t) => tickets.push(t),
            Err(e) => parse_errors.push(e),
        }
    }

    if !parse_errors.is_empty() {
        eprintln!("\nParse errors ({}):", parse_errors.len());
        for e in &parse_errors {
            eprintln!("  {e}");
        }
    }

    let known_ids: HashSet<&str> = tickets.iter().map(|t| t.front.id.as_str()).collect();

    println!(
        "Parsed {} tickets, verifying against ur...\n",
        tickets.len()
    );

    let (mismatches, missing, verified) = verify_tickets(&tickets, &known_ids, client).await?;

    print_summary(&mismatches, &missing, verified, tickets.len());

    if !missing.is_empty() || !mismatches.is_empty() {
        std::process::exit(1);
    }

    Ok(())
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().collect();

    let mut tickets_dir = PathBuf::from(".tickets");
    let mut server_addr = "http://127.0.0.1:42069".to_string();

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
            "--help" | "-h" => {
                println!("tk-verify: verify tk tickets were imported correctly into ur");
                println!();
                println!("Usage: tk-verify [OPTIONS]");
                println!();
                println!("Options:");
                println!("  -t, --tickets <DIR>    Tickets directory (default: .tickets)");
                println!(
                    "  -s, --server <ADDR>    Server address (default: http://127.0.0.1:42069)"
                );
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

    println!("tk -> ur import verification");
    println!("  Source:  {}", tickets_dir.display());
    println!("  Server:  {server_addr}");
    println!();

    let channel = Channel::from_shared(server_addr)
        .expect("invalid server address")
        .connect()
        .await;

    let channel = match channel {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Failed to connect to server: {e}");
            eprintln!("Is ur-server running? Start it with: ur server start");
            std::process::exit(1);
        }
    };

    let mut client = TicketServiceClient::new(channel);
    if let Err(e) = run_verify(&tickets_dir, &mut client).await {
        eprintln!("Verification failed: {e}");
        std::process::exit(1);
    }
}
