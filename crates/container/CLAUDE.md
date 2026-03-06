# container (Runtime Abstraction)

Pure library crate — no async, no network. Builds CLI arg vectors and shells out to `container` (Apple) or `docker`.

- `AppleRuntime` resolves `/tmp` → `/private/tmp` on host paths and uses `--publish-socket` for UDS (not `-v`)
- `AppleRuntime` passes `--arch` matching the host's `std::env::consts::ARCH` for native builds
- `DockerRuntime` uses `-v` for both volumes and socket mounts
- Integration test (`tests/lifecycle.rs`) requires a real container runtime — it builds/runs/stops/removes a container
