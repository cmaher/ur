# urd (Daemon)

Long-running daemon process. Listens on TCP via tonic gRPC, serves both host CLI (`ur`) and in-container workers (`workercmd` binaries).

- Runtime backend is selected via `UR_CONTAINER` env var or PATH detection (`container::runtime_from_env()`) — do not hardcode a backend
- Each RPC handler instantiates its own runtime via `runtime_from_env()` — there is no shared runtime state yet
- Main gRPC server runs on `127.0.0.1:$daemon_port` (TCP, default 42069); per-agent gRPC servers bind to the host gateway IP on an OS-assigned port
- `CoreServiceHandler` is `Clone` — keep it stateless or use `Arc` for shared state
- Config and constants are in the `ur_config` crate, re-exported via `urd::config`
- `stream::spawn_child_output_stream` is the shared helper for streaming child process output as `CommandOutput` gRPC frames
- `git_exec::validate_args` blocks `-C`, `--git-dir`, and `--work-tree` with errors
