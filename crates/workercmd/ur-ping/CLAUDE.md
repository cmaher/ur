# ur-ping (Worker Command)

Minimal container binary that pings the urd gRPC server to verify connectivity.

- Connects to `127.0.0.1:$UR_GRPC_PORT` (default port: `42069`) via tonic gRPC over TCP
- Prints the ping response message and exits 0 on success
- Used as a health check and connectivity test inside worker containers
