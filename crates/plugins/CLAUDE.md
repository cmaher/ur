# plugins

Plugin trait definitions and registries for the ur-free/ur-pro split. Contains only interfaces — no implementations.

- `types.rs` — `WorkerConfig` (volumes, env_vars) and `MigrationEntry` (database_name, migrations)
- `cli.rs` — `CliPlugin` trait (name, configure, modify_compose, register_cli, handle_cli) and `CliRegistry`
- `server.rs` — `ServerPlugin` trait (name, configure, modify_worker, migrations, register_grpc) and `ServerRegistry`
- `ui.rs` — `UiPlugin` trait (name, configure) and `UiRegistry`
- Each registry wraps `Vec<Box<dyn Trait>>` with `new()`, `register()`, and batch-apply methods
- All trait methods except `name()` have default no-op implementations
- Depends on `compose` crate for `ComposeFile` type and `tonic` for gRPC router
