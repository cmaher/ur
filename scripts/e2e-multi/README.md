# Multi-machine e2e VM (opt-in)

`vm.sh` manages `ur-e2e`, a Lima VM (Ubuntu 24.04, the host's arch) that plays
"machine B" in the multi-machine e2e suite (ur-ab3df). The suite is heavy, so it
is **not** part of the standard flow: neither `make ci` nor the pre-push hook
runs it. Unit and server integration tests are the primary coverage; use this
when multi-machine behavior breaks or while building multi-machine features.

## Requirements

`limactl` (Lima >= 2.0), `cargo-zigbuild` + `zig`, and a host Docker that has
the worker images (`make image`).

## Commands

```
scripts/e2e-multi/vm.sh setup [--fresh]   # create + provision (idempotent); --fresh deletes and recreates
scripts/e2e-multi/vm.sh sync              # cross-build ur, builderd, ur-workertools, workerd; install in /usr/local/bin; load worker images
scripts/e2e-multi/vm.sh reset             # wipe ur state in the VM: ~/.ur, containers labelled ur.instance, tmux 'ur' socket
scripts/e2e-multi/vm.sh shell             # shell in the VM
scripts/e2e-multi/vm.sh stop              # stop the VM (it persists; `setup` restarts it)
```

`make e2e-multi-vm ARGS="sync"` forwards to the script. `UR_E2E_IMAGE_TAG`
picks the host image tag to load (default `latest`). Behind a TLS-intercepting
proxy (e.g. Netskope), `setup` adds the PEM bundle in `UR_E2E_CA_CERTS`
(default `$SSL_CERT_FILE`) to the VM's trust store at create time; without it
the Docker install fails with a self-signed-certificate error. The bundle is
applied only when the VM is created: after a `setup` that failed without
`UR_E2E_CA_CERTS`/`SSL_CERT_FILE`, set one and rerun `vm.sh setup --fresh`.

## What it touches

- Only the Lima instance `ur-e2e`; every `limactl` call names it. Other Lima VMs are never touched.
- The VM has no host mounts, so nothing in it can reach the host's `~/.ur`.
  `reset` runs inside the VM only. On the host, `sync` only reads Docker images
  (`docker save`) and writes `target/`.
- Provisioning (`lima/ur-e2e.yaml`) installs Docker (get.docker.com), tmux, git
  and ca-certificates. It runs on every boot and is idempotent.

## Networking

From the VM the host is `host.lima.internal`. Guest ports bound on
`0.0.0.0`/`127.0.0.1` are forwarded to the host's `localhost` by Lima.
