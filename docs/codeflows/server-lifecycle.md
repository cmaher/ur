# Server Lifecycle (ur start / ur stop)

## Overview

`ur start` brings up the full stack: builderd (host daemon) + Docker Compose services
(ur-server, ur-squid, ur-qdrant). `ur stop` tears everything down in reverse.

## Start Flow

```
ur start
├── start_builderd(config)
│   ├── Acquire exclusive lock (~/.ur/builderd.lock)
│   ├── Check PID file (~/.ur/builderd.pid)
│   │   └── If alive → skip (already running)
│   ├── Resolve binary: sibling of `ur` executable, then PATH
│   ├── Spawn: builderd --port <builderd_port> --workspace <workspace>
│   ├── Detach into own process group (no Ctrl-C propagation)
│   ├── Write PID file
│   └── Release lock (drop)
│
└── compose.up()
    ├── generate_compose() → YAML string from ur.toml config
    ├── Write ~/.ur/docker-compose.yml
    ├── docker compose down (pre-cleanup of stale state)
    └── docker compose up -d --wait
        ├── ur-squid (forward proxy, infra + workers networks)
        └── ur-server (gRPC server, infra + workers networks)
            └── Connects to builderd via UR_BUILDERD_ADDR
```

## Stop Flow

```
ur stop
├── compose.down()
│   ├── docker compose down
│   └── Remove ~/.ur/docker-compose.yml
│
└── stop_builderd(config)
    ├── Read PID from ~/.ur/builderd.pid
    ├── SIGTERM the process
    └── Remove PID file
```

## Port Allocation

| Service | Default Port | Config Field | Derivation |
|---------|-------------|-------------|------------|
| ur-server (gRPC) | 42069 | `server_port` | Explicit or default |
| Worker gRPC | 42070 | `worker_port` | `server_port + 1` |
| builderd (gRPC) | 42071 | `builderd_port` | `server_port + 2` |

All ports derive from `server_port` when not explicitly set, ensuring test isolation
when using a custom `server_port`.

## Compose Generation

`generate_compose()` in `crates/ur/src/compose.rs` builds YAML programmatically from
`ur.toml` config (network names, container names, proxy hostname).
No static template — the compose file is generated fresh on every `ur start`.

Environment variables passed to `docker compose`:
- `UR_CONFIG` — host config directory (mounted as `/config` in server)
- `UR_WORKSPACE` — host workspace directory (mounted as `/workspace` in server)
- `UR_SERVER_PORT` — gRPC listen port for ur-server
- `UR_BUILDERD_PORT` — builderd port (server uses this to build `UR_BUILDERD_ADDR`)

The server container receives `UR_BUILDERD_ADDR=http://host.docker.internal:$UR_BUILDERD_PORT`
to reach builderd on the host via Docker's host gateway.

## Network Topology

```
Host (macOS)
├── builderd [:42071] ← gRPC from server container
│
└── Docker
    ├── infra network (bridge)
    │   ├── ur-server [:42069] ← gRPC from CLI + workers
    │   └── ur-squid [:3128] ← HTTP proxy for workers
    │
    └── workers network (bridge, internal)
        ├── ur-server (also on this network)
        ├── ur-squid (also on this network)
        └── worker containers (launched dynamically)
```

## Concurrency Safety

- `start_builderd()` uses an exclusive file lock (`builderd.lock`) to prevent races
  between concurrent `ur start` invocations
- PID file checked under lock — stale PIDs are cleaned up before spawning

## Key Files

- `crates/ur/src/builderd.rs` — builderd lifecycle (start/stop, PID management)
- `crates/ur/src/compose.rs` — ComposeManager + `generate_compose()`
- `crates/ur/src/main.rs` — `start_server()` / `stop_server()` orchestration
- `crates/ur_config/src/lib.rs` — port defaults, config parsing
