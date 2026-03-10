---
name: squid:investigate
description: Use when a worker container fails to reach an external service, install a marketplace extension, or when the user mentions squid logs, proxy denials, or blocked domains
---

# Squid Proxy Investigation

Diagnose and fix blocked domains in the ur squid forward proxy.

## Steps

1. **Read denied requests from squid access log:**
   ```bash
   docker exec ur-squid cat /var/log/squid/access.log | grep TCP_DENIED
   ```
   Logs are inside the container at `/var/log/squid/access.log` — `docker logs` shows nothing useful.

2. **Read current allowlist:**
   ```bash
   cat ~/.ur/squid/allowlist.txt
   ```

3. **Add missing domains** to `~/.ur/squid/allowlist.txt` (one per line). Skip telemetry domains (e.g. datadoghq) unless requested.

4. **Signal squid to reload:**
   ```bash
   docker exec ur-squid squid -k reconfigure
   ```
   Expect only a `WARNING: HTTP requires the use of Via` line. If you see `Can not open file` errors, verify with `docker exec ur-squid cat /etc/squid/allowlist.txt` and retry.

5. **Update the default allowlist** in `crates/ur/src/init.rs` (`DEFAULT_ALLOWLIST` constant) and its test (`allowlist_contains_anthropic`) so new setups include the domains.

## Common Mistakes

| Mistake | Fix |
|---------|-----|
| Using `docker logs ur-squid` | Logs go to files inside container, not stdout. Use `docker exec ... cat` |
| Adding telemetry/analytics domains | Skip unless explicitly requested — they're noise |
| Forgetting to update `init.rs` | Always update default allowlist + test assertions |
| Forgetting `squid -k reconfigure` | Squid won't pick up file changes without the signal |
