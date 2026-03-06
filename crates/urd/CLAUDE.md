# urd (Daemon)

Long-running daemon process. Listens on UDS, serves both host CLI (`ur`) and in-container workers (`agent_tools`).

- Runtime backend is selected via `UR_CONTAINER` env var or PATH detection (`container::runtime_from_env()`) — do not hardcode a backend
- Each RPC handler instantiates its own runtime via `runtime_from_env()` — there is no shared runtime state yet
- `BridgeServer` is `Clone` (required by tarpc) — keep it stateless or use `Arc` for shared state
