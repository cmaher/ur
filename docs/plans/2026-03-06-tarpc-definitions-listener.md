# tarpc Definitions & UDS Listener

**Ticket:** ur-8ii1
**Date:** 2026-03-06

## Summary

Define the `#[tarpc::service]` trait (`UrAgentBridge`) in `ur_rpc` with request/response structs for each method. Implement a `tokio::net::UnixListener` in `urd` that serves tarpc endpoints over a Unix Domain Socket.

## RPC Types

All request/response types derive `Serialize, Deserialize, Debug, Clone`.

### ask_human
- `AskHumanRequest { process_id: String, question: String }`
- `AskHumanResponse { answer: String }`

### exec_git
- `ExecGitRequest { process_id: String, args: Vec<String> }`
- `GitResponse { exit_code: i32, stdout: String, stderr: String }`

### report_status
- `ReportStatusRequest { process_id: String, status: String }`
- Returns `()`

### ticket_read
- `TicketReadRequest { ticket_id: String }`
- `TicketReadResponse { content: String }`

### ticket_spawn
- `TicketSpawnRequest { parent_id: String, title: String, description: String }`
- `TicketSpawnResponse { ticket_id: String }`

### ticket_note
- `TicketNoteRequest { ticket_id: String, note: String }`
- Returns `()`

## Service Trait

```rust
#[tarpc::service]
pub trait UrAgentBridge {
    async fn ask_human(req: AskHumanRequest) -> Result<AskHumanResponse, String>;
    async fn exec_git(req: ExecGitRequest) -> Result<GitResponse, String>;
    async fn report_status(req: ReportStatusRequest) -> Result<(), String>;
    async fn ticket_read(req: TicketReadRequest) -> Result<TicketReadResponse, String>;
    async fn ticket_spawn(req: TicketSpawnRequest) -> Result<TicketSpawnResponse, String>;
    async fn ticket_note(req: TicketNoteRequest) -> Result<(), String>;
}
```

All methods return `Result<T, String>` for uniform error handling. Errors are transport-independent application errors (e.g., "ticket not found"). tarpc handles transport-level errors separately.

## UDS Listener (urd)

- Bind `UnixListener` at `<socket_dir>/ur.sock`
- Accept loop: spawn a tokio task per connection
- Frame each `UnixStream` with `LengthDelimitedCodec`
- Codec: `tokio_serde::formats::Bincode`
- Serve via `tarpc::server::BaseChannel::with_defaults`
- Stub server impl returns placeholder values (real logic in later tickets)

## Dependencies

### Workspace Cargo.toml
- `tarpc = { version = "0.35", features = ["serde-transport", "tokio1"] }`
- `tokio-serde = { version = "0.9", features = ["bincode"] }`
- `tokio-util = { version = "0.7", features = ["codec"] }`

### ur_rpc
- `tarpc`, `serde`

### urd
- `tarpc`, `tokio-serde`, `tokio-util`, `ur_rpc`, `tokio`
