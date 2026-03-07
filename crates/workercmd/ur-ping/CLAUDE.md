# ur-ping (Worker Command)

Minimal container binary that pings the urd gRPC server to verify connectivity.

- Connects to `$UR_SOCKET` (default: `/var/run/ur/ur.sock`) via tonic gRPC over UDS
- Prints the ping response message and exits 0 on success
- Used as a health check and connectivity test inside worker containers
