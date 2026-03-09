# ur-server (Container Image)

Alpine Linux container image for the ur-server. Runs inside Docker alongside worker containers, managing them via the Docker socket.

- Build context is `containers/server/` -- all files copied into the image must live here
- Image is tagged `ur-server:latest` by convention
- The `ur-server` binary is cross-compiled for linux and staged into the build context before `docker build`
- Docker CLI is installed so the server can manage worker containers via the mounted Docker socket (`/var/run/docker.sock`, mounted at runtime via compose)
- Uses `tini` as PID 1 init to handle signal forwarding and zombie reaping
- Exposes port 42069 (default `daemon_port` from `ur_config::DEFAULT_DAEMON_PORT`)
- The gRPC server must bind to `0.0.0.0` (not `127.0.0.1`) when running in a container so other containers on the Docker network can reach it
