use serde::Serialize;

#[derive(Serialize)]
struct Param {
    name: &'static str,
    #[serde(rename = "type")]
    param_type: &'static str,
    required: bool,
    description: &'static str,
}

#[derive(Serialize)]
struct Command {
    name: &'static str,
    description: &'static str,
    params: Vec<Param>,
}

pub fn print_schema(subcommand: Option<&str>) {
    let commands = all_commands();
    match subcommand {
        Some(name) => {
            if let Some(cmd) = commands.iter().find(|c| c.name == name) {
                println!("{}", serde_json::to_string_pretty(cmd).unwrap());
            } else {
                eprintln!("unknown subcommand: {name}");
                eprintln!(
                    "available: {}",
                    commands
                        .iter()
                        .map(|c| c.name)
                        .collect::<Vec<_>>()
                        .join(", ")
                );
                std::process::exit(1);
            }
        }
        None => {
            println!("{}", serde_json::to_string_pretty(&commands).unwrap());
        }
    }
}

fn all_commands() -> Vec<Command> {
    vec![
        Command {
            name: "create",
            description: "Create a new ticket",
            params: vec![
                Param { name: "title", param_type: "string", required: true, description: "Ticket title (positional)" },
                Param { name: "--type", param_type: "string", required: false, description: "Ticket type: epic or task (default: task)" },
                Param { name: "--parent", param_type: "string", required: false, description: "Parent ticket ID (alias: --epic)" },
                Param { name: "--priority", param_type: "integer", required: false, description: "Priority, lower is higher (default: 0)" },
                Param { name: "--body", param_type: "string", required: false, description: "Ticket body text" },
            ],
        },
        Command {
            name: "list",
            description: "List tickets with optional filters",
            params: vec![
                Param { name: "--epic", param_type: "string", required: false, description: "Filter by parent epic ID (alias: --parent)" },
                Param { name: "--type", param_type: "string", required: false, description: "Filter by ticket type" },
                Param { name: "--status", param_type: "string", required: false, description: "Filter by status: open, in_progress, closed" },
            ],
        },
        Command {
            name: "show",
            description: "Show a ticket's full detail including metadata and activities",
            params: vec![
                Param { name: "id", param_type: "string", required: true, description: "Ticket ID (positional)" },
            ],
        },
        Command {
            name: "update",
            description: "Update a ticket's fields (only specified fields are changed)",
            params: vec![
                Param { name: "id", param_type: "string", required: true, description: "Ticket ID (positional)" },
                Param { name: "--title", param_type: "string", required: false, description: "New title" },
                Param { name: "--body", param_type: "string", required: false, description: "New body text" },
                Param { name: "--status", param_type: "string", required: false, description: "New status: open, in_progress, closed" },
                Param { name: "--priority", param_type: "integer", required: false, description: "New priority" },
                Param { name: "--type", param_type: "string", required: false, description: "New ticket type" },
                Param { name: "--parent", param_type: "string", required: false, description: "New parent ticket ID (alias: --epic)" },
                Param { name: "--force", param_type: "boolean", required: false, description: "Force update (e.g. close epic with open children)" },
            ],
        },
        Command {
            name: "set-meta",
            description: "Set a metadata key-value pair on a ticket",
            params: vec![
                Param { name: "id", param_type: "string", required: true, description: "Ticket ID (positional)" },
                Param { name: "key", param_type: "string", required: true, description: "Metadata key (positional)" },
                Param { name: "value", param_type: "string", required: true, description: "Metadata value (positional)" },
            ],
        },
        Command {
            name: "delete-meta",
            description: "Delete a metadata key from a ticket",
            params: vec![
                Param { name: "id", param_type: "string", required: true, description: "Ticket ID (positional)" },
                Param { name: "key", param_type: "string", required: true, description: "Metadata key to delete (positional)" },
            ],
        },
        Command {
            name: "add-activity",
            description: "Add an activity note to a ticket",
            params: vec![
                Param { name: "id", param_type: "string", required: true, description: "Ticket ID (positional)" },
                Param { name: "message", param_type: "string", required: true, description: "Activity message (positional)" },
                Param { name: "--meta", param_type: "string", required: false, description: "Metadata key=value pairs (repeatable)" },
            ],
        },
        Command {
            name: "list-activities",
            description: "List activities on a ticket",
            params: vec![
                Param { name: "id", param_type: "string", required: true, description: "Ticket ID (positional)" },
            ],
        },
        Command {
            name: "add-block",
            description: "Add a blocking dependency (blocked-by-id blocks id)",
            params: vec![
                Param { name: "id", param_type: "string", required: true, description: "Ticket ID that is blocked (positional)" },
                Param { name: "blocked_by_id", param_type: "string", required: true, description: "Ticket ID that is the blocker (positional)" },
            ],
        },
        Command {
            name: "remove-block",
            description: "Remove a blocking dependency",
            params: vec![
                Param { name: "id", param_type: "string", required: true, description: "Ticket ID that is blocked (positional)" },
                Param { name: "blocked_by_id", param_type: "string", required: true, description: "Ticket ID that is the blocker (positional)" },
            ],
        },
        Command {
            name: "add-link",
            description: "Add a bidirectional link between tickets",
            params: vec![
                Param { name: "id", param_type: "string", required: true, description: "First ticket ID (positional)" },
                Param { name: "linked_id", param_type: "string", required: true, description: "Second ticket ID (positional)" },
            ],
        },
        Command {
            name: "remove-link",
            description: "Remove a bidirectional link between tickets",
            params: vec![
                Param { name: "id", param_type: "string", required: true, description: "First ticket ID (positional)" },
                Param { name: "linked_id", param_type: "string", required: true, description: "Second ticket ID (positional)" },
            ],
        },
        Command {
            name: "dispatchable",
            description: "List dispatchable tickets for an epic (open children with no open blockers)",
            params: vec![
                Param { name: "epic_id", param_type: "string", required: true, description: "Epic ticket ID (positional)" },
            ],
        },
        Command {
            name: "status",
            description: "Print project status report (epic tree with open/closed counts)",
            params: vec![
                Param { name: "--project", param_type: "string", required: false, description: "Project key to filter by ID prefix (e.g. 'ur' shows ur-* tickets)" },
            ],
        },
    ]
}
