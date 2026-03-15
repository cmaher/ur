# ticket_client (Shared Ticket CLI Library)

Shared library crate providing clap subcommand definitions and gRPC client logic for the TicketService. Used by both the host CLI (`ur`) and worker tools (`workertools`).

- Exports `TicketArgs` clap enum with 13 subcommands matching all TicketService RPCs
- Exports `execute(args, client)` async function that dispatches clap args to gRPC calls
- Exports `format_ticket_detail` and `format_ticket_list` for consistent output formatting
- Depends on `ur_rpc` with the `ticket` feature for generated protobuf types
- No connection management — callers provide a `TicketServiceClient<Channel>`
- No state — pure functions only
