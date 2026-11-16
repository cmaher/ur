#!/usr/bin/env bash
# Start the ci-postgres container for tests if it is not already up.
# The host port is UR_CI_POSTGRES_PORT (default 5433); the tests read the same variable.
set -euo pipefail

PORT="${UR_CI_POSTGRES_PORT:-5433}"

# The port ci-postgres publishes, or nothing when the container does not exist.
published_port() {
    docker inspect -f '{{with (index .HostConfig.PortBindings "5432/tcp")}}{{(index . 0).HostPort}}{{end}}' \
        ci-postgres 2>/dev/null || true
}

ready() {
    docker exec ci-postgres pg_isready -U ur >/dev/null 2>&1
}

EXISTING="$(published_port)"
if [ -n "$EXISTING" ] && [ "$EXISTING" != "$PORT" ]; then
    echo "ci-postgres publishes port $EXISTING, not $PORT; recreating it"
    docker rm -f ci-postgres >/dev/null
    EXISTING=""
fi

# Fast path: our own container is up. A listener on the port is not enough: it may be
# another service.
if [ -n "$EXISTING" ] && ready; then
    exit 0
fi

if [ -n "$EXISTING" ]; then
    echo "ci-postgres exists but is not ready; starting"
    docker start ci-postgres >/dev/null
else
    if (exec 3<>"/dev/tcp/localhost/$PORT") 2>/dev/null; then
        echo "port $PORT is in use by something other than ci-postgres;" \
            "free it or set UR_CI_POSTGRES_PORT to another port"
        exit 1
    fi
    echo "creating ci-postgres"
    docker run -d --name ci-postgres \
        -e POSTGRES_USER=ur \
        -e POSTGRES_PASSWORD=ur \
        -e POSTGRES_DB=postgres \
        -p "$PORT:5432" \
        --shm-size=512m \
        postgres:17 >/dev/null
fi
echo "Waiting for ci-postgres to be ready..."
for _ in $(seq 1 30); do
    if ready; then
        echo "ci-postgres is ready on localhost:$PORT"
        exit 0
    fi
    sleep 1
done
echo "ci-postgres failed to become ready"
exit 1
