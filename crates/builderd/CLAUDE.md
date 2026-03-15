# builderd

Builder daemon for executing commands on the macOS host. Receives already-validated
requests from ur-server and spawns processes locally.

- Standalone binary, runs natively on host (not containerized)
- tonic gRPC server on `127.0.0.1:<hostd_port>`
- Trusts ur-server completely — no command validation
- Started/stopped by `ur start`/`ur stop`, PID tracked at `~/.ur/hostd.pid`
