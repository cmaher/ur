# ur (Host CLI)

Runs on the host macOS system. Connects to `urd` via tarpc over UDS at `$UR_CONFIG/ur.sock` (default `~/.ur/ur.sock`). Use `--socket` to override.

- Container build context path is resolved relative to `current_dir()`, not the binary location
- `process launch` both builds the image and runs the container in sequence — don't split into separate commands
- `process stop` calls both `container_stop` and `container_rm` — stopping without removing is not exposed to users
