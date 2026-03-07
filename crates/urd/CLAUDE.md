# urd (Daemon)

Long-running daemon process. Listens on UDS via tonic gRPC, serves both host CLI (`ur`) and in-container workers (`workercmd` binaries).

- Runtime backend is selected via `UR_CONTAINER` env var or PATH detection (`container::runtime_from_env()`) — do not hardcode a backend
- Each RPC handler instantiates its own runtime via `runtime_from_env()` — there is no shared runtime state yet
- Main gRPC server runs on `$UR_CONFIG/ur-grpc.sock`; per-agent gRPC servers are spawned on `$UR_CONFIG/<process_id>/ur.sock`
- `CoreServiceHandler` is `Clone` — keep it stateless or use `Arc` for shared state
