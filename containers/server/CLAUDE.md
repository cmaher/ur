# ur-server (Container Image)

Alpine Linux container image for the ur-server. Runs inside Docker alongside worker containers, managing them through builderd on the host.

- Build context is `containers/server/` -- all files copied into the image must live here
- Image is tagged `ur-server:latest` by convention
- The `ur-server` binary is cross-compiled for linux and staged into the build context before `docker build`
- **No Docker socket is mounted.** Every container operation (launch, stop, exec, inspect) goes to builderd on the host over gRPC — see `crates/server/CLAUDE.md`. The Docker CLI in the image is a debugging convenience only; code that shells out to it from this container will fail against an absent daemon, and a failure must never be read as "container is gone"
- Uses `tini` as PID 1 init to handle signal forwarding and zombie reaping
- Exposes port 12321 (default `server_port` from `ur_config::DEFAULT_SERVER_PORT`)
- The gRPC server must bind to `0.0.0.0` (not `127.0.0.1`) when running in a container so other containers on the Docker network can reach it
