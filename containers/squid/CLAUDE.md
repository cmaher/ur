# squid (Forward Proxy Container)

Alpine-based Squid forward proxy for restricting container network access.

- Config is NOT baked into the image — mounted at runtime from `$UR_CONFIG/squid/` to `/etc/squid/` (read-only)
- `squid -N` runs in foreground (no daemon mode)
- Runs as a compose service alongside urd; urd writes config, signals reload
- Allowlist updates: urd rewrites `allowlist.txt`, then `docker exec ur-squid squid -k reconfigure`
- Image is tagged `ur-squid:latest` by convention
- Workers reach the proxy via Docker DNS: `ur-squid:3128`
- On the `internal` network (shared with workers) and `external` network (for upstream)
