#!/usr/bin/env bash
set -euo pipefail

# Lifecycle of the `ur-e2e` Lima VM ("machine B" of the opt-in multi-machine
# e2e suite). Never run by `make ci` or the pre-push hook; see README.md.
#
# Every limactl call names $VM explicitly so no other Lima instance is touched.

VM=ur-e2e
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
TEMPLATE="$SCRIPT_DIR/lima/ur-e2e.yaml"
BINARIES=(ur builderd ur-workertools workerd)
MIN_TMUX=3.4
IMAGE_TAG="${UR_E2E_IMAGE_TAG:-latest}"

# shellcheck source-path=SCRIPTDIR
source "$REPO_ROOT/scripts/build/agent-images.sh"

usage() {
    cat <<EOF
Usage: $(basename "$0") <command>

  setup [--fresh]  create and provision the $VM VM (--fresh deletes it first)
  sync             cross-build ${BINARIES[*]}, install them in the VM, load worker images
  reset            wipe ur state inside the VM (~/.ur, ur containers, tmux 'ur' socket)
  shell            open a shell in the VM
  stop             stop the VM
EOF
}

vm_status() {
    limactl list "$VM" --format '{{.Status}}' 2>/dev/null || true
}

vm_exec() {
    limactl shell --workdir / "$VM" "$@"
}

require_running() {
    if [ "$(vm_status)" != "Running" ]; then
        echo "$VM is not running; run '$0 setup' first" >&2
        exit 1
    fi
}

cmd_setup() {
    if [ "${1:-}" = "--fresh" ] && [ -n "$(vm_status)" ]; then
        echo "Deleting $VM (--fresh)"
        limactl delete --force "$VM"
    elif [ -n "${1:-}" ] && [ "$1" != "--fresh" ]; then
        usage >&2
        exit 2
    fi

    case "$(vm_status)" in
        "")
            echo "Creating $VM from $TEMPLATE"
            # TLS-intercepting proxies: trust the same CA bundle as the host.
            local ca="${UR_E2E_CA_CERTS:-${SSL_CERT_FILE:-}}" set_args=()
            if [ -n "$ca" ]; then
                echo "Adding CA certificates from $ca"
                set_args=(--set ".caCerts.files = [\"$ca\"]")
            fi
            limactl create --tty=false --name "$VM" ${set_args[@]+"${set_args[@]}"} "$TEMPLATE"
            limactl start --tty=false "$VM"
            ;;
        Running) echo "$VM already running" ;;
        *) limactl start --tty=false "$VM" ;;
    esac

    verify_provisioning
}

verify_provisioning() {
    vm_exec sudo docker version --format 'docker {{.Server.Version}}'
    local tmux_version
    tmux_version="$(vm_exec tmux -V | awk '{print $2}')"
    if [ "$(printf '%s\n%s\n' "$MIN_TMUX" "$tmux_version" | sort -V | head -n1)" != "$MIN_TMUX" ]; then
        echo "tmux $tmux_version in $VM is older than $MIN_TMUX" >&2
        exit 1
    fi
    echo "tmux $tmux_version"
    vm_exec git --version
}

cmd_sync() {
    require_running
    local arch target
    arch="$(vm_exec uname -m)"
    case "$arch" in
        aarch64) target=aarch64-unknown-linux-gnu ;;
        x86_64) target=x86_64-unknown-linux-gnu ;;
        *) echo "Unsupported VM architecture: $arch" >&2; exit 1 ;;
    esac

    # The zig linker opens many .rlib files at once (see scripts/build/stage.sh).
    ulimit -n 65536 2>/dev/null || true
    local pkgs=() bin
    for bin in "${BINARIES[@]}"; do pkgs+=(-p "$bin"); done
    echo "Cross-compiling ${BINARIES[*]} for $target"
    (cd "$REPO_ROOT" && cargo zigbuild --release --target "$target" "${pkgs[@]}")

    vm_exec mkdir -p /tmp/ur-e2e-bin
    for bin in "${BINARIES[@]}"; do
        limactl copy "$REPO_ROOT/target/$target/release/$bin" "$VM:/tmp/ur-e2e-bin/$bin"
    done
    vm_exec sudo sh -c 'install -m 0755 /tmp/ur-e2e-bin/* /usr/local/bin/ && rm -rf /tmp/ur-e2e-bin'
    echo "Installed ${BINARIES[*]} in $VM:/usr/local/bin"

    local images=() entry tag
    for entry in "${AGENT_IMAGES[@]}"; do
        IFS=':' read -r _dir tag _rest <<< "$entry"
        images+=("$tag:$IMAGE_TAG")
    done
    echo "Loading ${images[*]} into $VM's Docker"
    docker save "${images[@]}" | vm_exec sudo docker load
}

cmd_reset() {
    require_running
    # ur.instance is set on every container builderd launches.
    vm_exec bash -s <<'EOF'
set -euo pipefail
sudo pkill -x builderd || true
sudo pkill -x workerd || true
ids="$(sudo docker ps -aq --filter label=ur.instance)"
if [ -n "$ids" ]; then
    # shellcheck disable=SC2086
    sudo docker rm -f $ids >/dev/null
fi
echo "removed $(printf '%s' "$ids" | grep -c . || true) ur containers"
tmux -L ur kill-server 2>/dev/null || true
rm -rf "${UR_CONFIG:-$HOME/.ur}"
echo "cleared ${UR_CONFIG:-$HOME/.ur} and tmux socket 'ur'"
EOF
}

case "${1:-}" in
    setup) shift; cmd_setup "$@" ;;
    sync) cmd_sync ;;
    reset) cmd_reset ;;
    shell) require_running; limactl shell "$VM" ;;
    stop) limactl stop "$VM" ;;
    *) usage >&2; exit 2 ;;
esac
