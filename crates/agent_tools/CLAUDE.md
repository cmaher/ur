# agent_tools (Worker CLI)

Runs inside containers, not on the host. This binary is the container's interface to `urd` via tarpc over UDS.

- Will be cross-compiled and copied into the container image at build time
- Must connect to the UDS socket mounted into the container (currently `/var/run/ur.sock`)
- Commands are stubs — implementations will use `UrAgentBridgeClient` like `ur` does
