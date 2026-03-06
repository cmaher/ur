# agent_tools (Worker CLI)

Runs inside containers, not on the host. This binary is the container's interface to `urd` via tarpc over UDS.

- Will be cross-compiled and copied into the container image at build time
- Connects to `$UR_CONFIG/ur.sock` (default `~/.ur/ur.sock`); override with `--socket` or `UR_SOCKET`
- Commands are stubs — implementations will use `UrAgentBridgeClient` like `ur` does
