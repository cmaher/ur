# ur-ping (Worker Command)

Minimal container binary that pings the urd gRPC server to verify connectivity.

- Connects to `$URD_ADDR` (host:port) via tonic gRPC over TCP
- `URD_ADDR` env var is **required** — the binary panics if it is not set
- Prints the ping response message and exits 0 on success
- Used as a health check and connectivity test inside worker containers
