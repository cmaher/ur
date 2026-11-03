#!/usr/bin/env bash
# Start the ci-postgres container for tests (port 5433) if not already up.
set -euo pipefail

# Fast path: if something is already listening on :5433, assume ci-postgres is up.
if (exec 3<>/dev/tcp/localhost/5433) 2>/dev/null; then
    exit 0
fi
if docker inspect ci-postgres >/dev/null 2>&1; then
    echo "ci-postgres exists but not listening; starting"
    docker start ci-postgres >/dev/null 2>&1 || true
else
    echo "creating ci-postgres"
    docker run -d --name ci-postgres \
        -e POSTGRES_USER=ur \
        -e POSTGRES_PASSWORD=ur \
        -e POSTGRES_DB=postgres \
        -p 5433:5432 \
        --shm-size=512m \
        postgres:17
fi
echo "Waiting for ci-postgres to be ready..."
for _ in $(seq 1 30); do
    if docker exec ci-postgres pg_isready -U ur >/dev/null 2>&1; then
        echo "ci-postgres is ready on localhost:5433"
        exit 0
    fi
    sleep 1
done
echo "ci-postgres failed to become ready"
exit 1
