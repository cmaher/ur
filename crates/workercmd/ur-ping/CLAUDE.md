# ur-ping (Worker Command)

Minimal container binary that pings the urd gRPC server to verify connectivity.

- Connects to `$UR_GRPC_HOST:$UR_GRPC_PORT` via tonic gRPC over TCP
- `UR_GRPC_HOST` and `UR_GRPC_PORT` env vars are **required** — the binary panics if they are not set
- Prints the ping response message and exits 0 on success
- Used as a health check and connectivity test inside worker containers
