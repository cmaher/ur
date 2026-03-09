# Squid Proxy Replacement Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Replace the custom hyper-based forward proxy with an Alpine Squid container, managed via Docker Compose alongside urd, with dynamic allowlist updates via config file writes + `squid -k reconfigure`.

**Architecture:** Squid runs as a compose service (`ur-squid`) on the same Docker network as urd and workers. urd writes Squid config files to `$UR_CONFIG/squid/` (mounted read-write in urd, read-only in Squid). Allowlist changes are applied by rewriting the ACL file and running `docker exec ur-squid squid -k reconfigure` from urd (which has the Docker socket). Workers reach Squid via Docker DNS (`ur-squid:3128`). Docker network isolation (`internal: true` on the worker network) replaces the old pf firewall.

**Tech Stack:** Alpine Linux + Squid (apk), Docker Compose, Rust (container crate, urd)

**Ticket:** ur-as7t (epic, replaces ur-usca), closes ur-5hsk (runtime allowlist update)

**Relationship to ur-19t0 (OrbStack migration):**

- ur-3v3e (compose template) — must include `ur-squid` service. This plan creates the Squid image and documents the compose service definition; ur-3v3e wires it into the actual template.
- ur-14fp (urd container image) — urd container needs Docker CLI access to `docker exec` Squid for reconfigure. Already planned (Alpine + Docker CLI).
- ur-jjcf (build scripts) — Squid image build is added here.
- ur-8mrq (compose-start CLI) — `ur` starts the full compose stack including Squid. No special handling needed.

Tasks 1-2 of this plan have **no dependencies** on ur-19t0 and can land first. Task 3 (wiring into urd main + compose) depends on ur-3v3e and ur-14fp.

---

## How `squid -k reconfigure` Works

- Sends SIGHUP to the running Squid master process
- Squid re-reads **all** config files (squid.conf + referenced ACL files)
- Active connections are **preserved** — no disruption
- New connections use the updated rules
- Takes milliseconds
- From urd container: `docker exec ur-squid squid -k reconfigure` (urd has Docker socket)

---

## Squid Config Design

Two files in `$UR_CONFIG/squid/`:

**`squid.conf`** (static, written once at init):
```
# Ur forward proxy — managed by urd. Do not edit manually.
http_port 3128

acl allowed_domains dstdomain "/etc/squid/allowlist.txt"
acl CONNECT method CONNECT

http_access allow CONNECT allowed_domains
http_access allow allowed_domains
http_access deny all

access_log stdio:/dev/stdout
cache_log stdio:/dev/stderr
cache deny all
via off
forwarded_for delete
```

**`allowlist.txt`** (dynamic, rewritten on updates):
```
api.anthropic.com
```

Note: Squid `dstdomain` ACLs match exact domain names. No leading dot (that would also match subdomains), matching current ProxyManager behavior.

---

## Compose Service Definition (for ur-3v3e)

This is the service definition that ur-3v3e should include in the compose template:

```yaml
services:
  ur-squid:
    image: ur-squid:latest
    container_name: ur-squid
    volumes:
      - ${UR_CONFIG:-~/.ur}/squid:/etc/squid:ro
    networks:
      - internal
      - external
    restart: unless-stopped

  urd:
    # ... existing definition from ur-14fp ...
    volumes:
      - ${UR_CONFIG:-~/.ur}:/config
      - /var/run/docker.sock:/var/run/docker.sock
    networks:
      - internal

  # workers are launched dynamically by urd via docker run --network=internal

networks:
  internal:
    internal: true    # no external gateway — workers can only reach squid and urd
  external:           # squid uses this for upstream connections
```

---

### Task 1: Squid Container Image

**Files:**
- Create: `containers/squid/Dockerfile`
- Create: `containers/squid/CLAUDE.md`
- Modify: `scripts/build/container-image.sh`

**Step 1: Create the Squid Dockerfile**

Create `containers/squid/Dockerfile`:

```dockerfile
FROM alpine:3.21
RUN apk add --no-cache squid
EXPOSE 3128
CMD ["squid", "-N", "-f", "/etc/squid/squid.conf"]
```

`-N` = foreground (no daemon mode), correct for containers.

**Step 2: Create `containers/squid/CLAUDE.md`**

```markdown
# squid (Forward Proxy Container)

Alpine-based Squid forward proxy for restricting container network access.

- Config is NOT baked into the image — mounted at runtime from `$UR_CONFIG/squid/` to `/etc/squid/` (read-only)
- `squid -N` runs in foreground (no daemon mode)
- Runs as a compose service alongside urd; urd writes config, signals reload
- Allowlist updates: urd rewrites `allowlist.txt`, then `docker exec ur-squid squid -k reconfigure`
- Image is tagged `ur-squid:latest` by convention
- Workers reach the proxy via Docker DNS: `ur-squid:3128`
- On the `internal` network (shared with workers) and `external` network (for upstream)
```

**Step 3: Add Squid image to the build script**

Refactor `scripts/build/container-image.sh` — `build_image` currently uses a hardcoded `$CONTEXT`. Change it to accept context as a parameter:

```bash
build_image() {
    local tag="$1"
    local dockerfile="$2"
    local context="$3"
    echo "Building $tag..."
    if command -v docker >/dev/null 2>&1; then
        docker build -t "$tag" -f "$dockerfile" "$context"
    elif command -v nerdctl >/dev/null 2>&1; then
        nerdctl build -t "$tag" -f "$dockerfile" "$context"
    else
        echo "Warning: no container runtime found, skipping image build"
        exit 1
    fi
}

WORKER_CONTEXT=containers/claude-worker

build_image ur-worker-base:latest "$WORKER_CONTEXT/Dockerfile.base" "$WORKER_CONTEXT"
echo "Base image built: ur-worker-base:latest"

build_image ur-worker:latest "$WORKER_CONTEXT/Dockerfile" "$WORKER_CONTEXT"
echo "Worker image built: ur-worker:latest"

build_image ur-squid:latest containers/squid/Dockerfile containers/squid
echo "Squid proxy image built: ur-squid:latest"
```

**Step 4: Build and verify the image**

Run: `docker build -t ur-squid:latest containers/squid/`
Expected: Image builds successfully, ~10MB.

Run: `docker run --rm ur-squid:latest squid -v`
Expected: Prints Squid version.

**Step 5: Commit**

```
feat(squid): add Alpine Squid container image

ur-usca
```

---

### Task 2: SquidManager — Config Writing and Allowlist Management

Replace `proxy.rs` (the custom hyper proxy) with a `SquidManager` that manages Squid config files and signals reconfigure.

**Files:**
- Rewrite: `crates/urd/src/proxy.rs`
- Modify: `crates/urd/src/lib.rs` (re-export SquidManager instead of ProxyManager)
- Modify: `crates/ur_config/src/lib.rs` (update ProxyConfig, add squid_dir helper, add constants)

**Step 1: Update ur_config**

In `crates/ur_config/src/lib.rs`:

Replace `DEFAULT_PROXY_PORT` (42070) with:

```rust
/// Default hostname for the Squid proxy container on the Docker network.
pub const DEFAULT_PROXY_HOSTNAME: &str = "ur-squid";

/// Squid listening port inside the container (standard Squid default).
pub const SQUID_PORT: u16 = 3128;
```

Update `ProxyConfig` — remove `port` (Squid always uses 3128 internally), add `hostname`:

```rust
pub struct ProxyConfig {
    /// Hostname containers use to reach the proxy via Docker DNS (default: "ur-squid").
    pub hostname: String,
    /// Domain allowlist — only these hosts may be reached through the proxy.
    pub allowlist: Vec<String>,
}
```

Update `RawProxyConfig` to match (remove `port`, add optional `hostname`).

Add to `Config` impl:

```rust
/// Path to the Squid config directory: `$UR_CONFIG/squid/`.
pub fn squid_dir(&self) -> PathBuf {
    self.config_dir.join("squid")
}
```

Update all default resolution code and tests. The `allowlist_set()` method can be removed (SquidManager uses `Vec<String>`, not `HashSet`).

**Step 2: Write failing tests for SquidManager**

In `crates/urd/src/proxy.rs`, write tests:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn test_allowlist() -> Vec<String> {
        vec!["api.anthropic.com".into(), "example.com".into()]
    }

    #[test]
    fn writes_squid_conf() {
        let tmp = TempDir::new().unwrap();
        let manager = SquidManager::new(tmp.path().to_path_buf(), test_allowlist());
        manager.write_config().unwrap();

        let conf = std::fs::read_to_string(tmp.path().join("squid.conf")).unwrap();
        assert!(conf.contains("http_port 3128"));
        assert!(conf.contains("allowlist.txt"));
        assert!(conf.contains("http_access deny all"));
    }

    #[test]
    fn writes_allowlist_file() {
        let tmp = TempDir::new().unwrap();
        let manager = SquidManager::new(tmp.path().to_path_buf(), test_allowlist());
        manager.write_config().unwrap();

        let allowlist = std::fs::read_to_string(tmp.path().join("allowlist.txt")).unwrap();
        assert!(allowlist.contains("api.anthropic.com"));
        assert!(allowlist.contains("example.com"));
    }

    #[test]
    fn update_allowlist_rewrites_file() {
        let tmp = TempDir::new().unwrap();
        let manager = SquidManager::new(tmp.path().to_path_buf(), test_allowlist());
        manager.write_config().unwrap();

        manager.update_allowlist(vec!["new.example.com".into()]).unwrap();

        let content = std::fs::read_to_string(tmp.path().join("allowlist.txt")).unwrap();
        assert!(content.contains("new.example.com"));
        assert!(!content.contains("api.anthropic.com"));
    }

    #[test]
    fn add_domain_appends() {
        let tmp = TempDir::new().unwrap();
        let manager = SquidManager::new(tmp.path().to_path_buf(), test_allowlist());
        manager.write_config().unwrap();

        manager.add_domain("new.example.com").unwrap();

        let domains = manager.list_domains();
        assert!(domains.contains(&"new.example.com".to_string()));
        assert!(domains.contains(&"api.anthropic.com".to_string()));
    }

    #[test]
    fn add_domain_deduplicates() {
        let tmp = TempDir::new().unwrap();
        let manager = SquidManager::new(tmp.path().to_path_buf(), test_allowlist());
        manager.write_config().unwrap();

        manager.add_domain("api.anthropic.com").unwrap();

        assert_eq!(manager.list_domains().len(), 2);
    }

    #[test]
    fn remove_domain() {
        let tmp = TempDir::new().unwrap();
        let manager = SquidManager::new(tmp.path().to_path_buf(), test_allowlist());
        manager.write_config().unwrap();

        manager.remove_domain("example.com").unwrap();

        let domains = manager.list_domains();
        assert!(!domains.contains(&"example.com".to_string()));
        assert!(domains.contains(&"api.anthropic.com".to_string()));
    }

    #[test]
    fn list_domains_returns_current() {
        let tmp = TempDir::new().unwrap();
        let manager = SquidManager::new(tmp.path().to_path_buf(), test_allowlist());
        assert_eq!(manager.list_domains().len(), 2);
    }
}
```

**Step 3: Implement SquidManager**

Replace contents of `crates/urd/src/proxy.rs`:

```rust
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use anyhow::{Context, Result};
use tracing::info;

/// Squid proxy container name on the Docker network.
pub const SQUID_CONTAINER_NAME: &str = "ur-squid";

const SQUID_CONF: &str = "\
# Ur forward proxy — managed by urd. Do not edit manually.
http_port 3128

acl allowed_domains dstdomain \"/etc/squid/allowlist.txt\"
acl CONNECT method CONNECT

http_access allow CONNECT allowed_domains
http_access allow allowed_domains
http_access deny all

access_log stdio:/dev/stdout
cache_log stdio:/dev/stderr
cache deny all
via off
forwarded_for delete
";

/// Manages Squid proxy config files and runtime allowlist.
///
/// Config files live in a host directory (`$UR_CONFIG/squid/`) mounted into the
/// Squid container at `/etc/squid/`. The Squid container itself is managed by
/// Docker Compose — this manager only handles config and reconfigure signals.
///
/// Allowlist changes: rewrite `allowlist.txt`, then `signal_reconfigure()` to
/// tell Squid to re-read its config without restarting.
#[derive(Clone)]
pub struct SquidManager {
    config_dir: PathBuf,
    allowlist: Arc<RwLock<Vec<String>>>,
}

impl SquidManager {
    pub fn new(config_dir: PathBuf, allowlist: Vec<String>) -> Self {
        Self {
            config_dir,
            allowlist: Arc::new(RwLock::new(allowlist)),
        }
    }

    /// Write `squid.conf` and `allowlist.txt` to the config directory.
    /// Called once at urd startup, before compose brings up the Squid service.
    pub fn write_config(&self) -> Result<()> {
        std::fs::create_dir_all(&self.config_dir)
            .with_context(|| format!("create squid config dir: {}", self.config_dir.display()))?;

        std::fs::write(self.config_dir.join("squid.conf"), SQUID_CONF)
            .context("write squid.conf")?;

        self.write_allowlist_file()?;
        info!(dir = %self.config_dir.display(), "squid config written");
        Ok(())
    }

    /// Replace the entire allowlist and write to disk.
    pub fn update_allowlist(&self, domains: Vec<String>) -> Result<()> {
        *self.allowlist.write().expect("allowlist lock poisoned") = domains;
        self.write_allowlist_file()
    }

    /// Add a domain to the allowlist and write to disk. No-op if already present.
    pub fn add_domain(&self, domain: &str) -> Result<()> {
        {
            let mut list = self.allowlist.write().expect("allowlist lock poisoned");
            if !list.iter().any(|d| d == domain) {
                list.push(domain.to_string());
            }
        }
        self.write_allowlist_file()
    }

    /// Remove a domain from the allowlist and write to disk.
    pub fn remove_domain(&self, domain: &str) -> Result<()> {
        self.allowlist
            .write()
            .expect("allowlist lock poisoned")
            .retain(|d| d != domain);
        self.write_allowlist_file()
    }

    /// Return a snapshot of the current allowlist.
    pub fn list_domains(&self) -> Vec<String> {
        self.allowlist
            .read()
            .expect("allowlist lock poisoned")
            .clone()
    }

    /// Signal the Squid container to re-read config files.
    /// Sends `squid -k reconfigure` via `docker exec`. Active connections are
    /// preserved; new connections use the updated allowlist.
    pub fn signal_reconfigure(&self) -> Result<()> {
        let rt = container::runtime_from_env();
        let cid = container::ContainerId(SQUID_CONTAINER_NAME.to_string());
        let opts = container::ExecOpts {
            command: vec!["squid".into(), "-k".into(), "reconfigure".into()],
            workdir: None,
        };
        let output = rt
            .exec(&cid, &opts)
            .context("signal squid reconfigure")?;
        if output.exit_code != 0 {
            anyhow::bail!("squid reconfigure failed: {}", output.stderr);
        }
        info!("squid reconfigure signaled");
        Ok(())
    }

    fn write_allowlist_file(&self) -> Result<()> {
        let list = self.allowlist.read().expect("allowlist lock poisoned");
        let content = list.join("\n") + "\n";
        std::fs::write(self.config_dir.join("allowlist.txt"), content)
            .context("write allowlist.txt")
    }
}
```

**Step 4: Update lib.rs re-exports**

In `crates/urd/src/lib.rs`, replace `ProxyManager` with `SquidManager`:
- Change `pub use proxy::ProxyManager;` → `pub use proxy::SquidManager;`

**Step 5: Run tests**

Run: `cargo test -p urd proxy`
Expected: All tests pass.

**Step 6: Commit**

```
feat(proxy): replace custom hyper proxy with SquidManager

SquidManager writes Squid config files and signals reconfigure via
docker exec. No more in-process proxy — Squid runs as a compose service.

ur-usca
```

---

### Task 3: Wire SquidManager into urd and Update ProcessManager

Replace the old ProxyManager usage in urd main and update ProcessManager to point workers at Squid.

**Depends on:** ur-14fp (urd container image), ur-3v3e (compose template includes ur-squid service)

**Files:**
- Modify: `crates/urd/src/main.rs`
- Modify: `crates/urd/src/process.rs` (update proxy env vars)
- Modify: `crates/urd/Cargo.toml` (remove unused hyper proxy deps)

**Step 1: Update main.rs**

Replace the proxy startup block (lines 43-48):

```rust
// Write Squid config files (compose manages the container lifecycle).
let squid_manager = SquidManager::new(cfg.squid_dir(), cfg.proxy.allowlist.clone());
squid_manager.write_config()?;
```

Remove `use std::net::SocketAddr;` if no longer needed (check — it's also used for the gRPC bind address on line 56).

**Step 2: Update process.rs**

Simplify `proxy_env_vars` — use Squid hostname + constant port:

```rust
fn proxy_env_vars(proxy_hostname: &str) -> Vec<(String, String)> {
    let proxy_url = format!("http://{proxy_hostname}:{}", ur_config::SQUID_PORT);
    vec![
        ("HTTP_PROXY".into(), proxy_url.clone()),
        ("HTTPS_PROXY".into(), proxy_url),
        ("NO_PROXY".into(), String::new()),
    ]
}
```

Update the call site in `run_and_record` (line 132):

```rust
env_vars.extend(proxy_env_vars(&config.proxy_hostname));
```

Add `proxy_hostname: String` to `ProcessConfig` struct. Set it from `cfg.proxy.hostname` in grpc.rs `process_launch`.

Remove `proxy: ProxyConfig` from `ProcessManager` fields, constructor, and all test helpers. ProcessManager no longer needs proxy config — the hostname comes through `ProcessConfig` per-launch.

**Step 3: Update proxy_env_vars tests**

```rust
#[test]
fn proxy_env_vars_uses_squid_hostname() {
    let vars = proxy_env_vars("ur-squid");
    assert_eq!(vars[0], ("HTTP_PROXY".into(), "http://ur-squid:3128".into()));
    assert_eq!(vars[1], ("HTTPS_PROXY".into(), "http://ur-squid:3128".into()));
    assert_eq!(vars[2], ("NO_PROXY".into(), String::new()));
}

#[test]
fn proxy_env_vars_uses_http_scheme_for_https() {
    let vars = proxy_env_vars("ur-squid");
    assert!(vars[1].1.starts_with("http://"));
}
```

**Step 4: Remove unused hyper proxy dependencies**

Check `crates/urd/Cargo.toml` for deps only used by the old proxy. These were used by `proxy.rs` and likely not by tonic:
- `hyper` (check — tonic uses hyper internally but may not re-export features we need)
- `hyper-util`
- `http-body-util`
- `http`
- `bytes`

For each: `grep -r` the crate to see if any other file uses it. Remove unused ones.

**Step 5: Run all tests**

Run: `cargo test -p urd`
Expected: All pass.

Run: `cargo make clippy`
Expected: Clean.

**Step 6: Commit**

```
feat(proxy): wire SquidManager into urd, update ProcessManager

urd writes Squid config at startup. Workers get HTTP_PROXY=ur-squid:3128.
Removes ProxyConfig from ProcessManager and unused hyper dependencies.

ur-usca
```

---

### Task 4: Runtime Proxy Domain Management via ur CLI (Host-Side)

`ur` CLI manages the Squid domain allowlist directly on the host. No gRPC — intentionally not exposed to agents. Closes ur-5hsk.

**Files:**
- Modify: `crates/ur/src/main.rs` (add CLI subcommand)
- Possibly: shared allowlist file helpers in `crates/ur_config/` or a small utility module

**Step 1: Add CLI subcommand to ur**

In `crates/ur/src/main.rs`, add:

```
ur proxy allow <domain>
ur proxy block <domain>
ur proxy list
```

Implementation:
- Read `$UR_CONFIG/squid/allowlist.txt` (one domain per line)
- For `allow`: append domain if not already present, write file
- For `block`: filter out domain, write file
- For `list`: print domains to stdout
- After `allow`/`block`: run `docker exec ur-squid squid -k reconfigure` to apply changes
- Print resulting domain list after mutations

The config path comes from `Config::squid_dir()` (ur_config).

**Step 2: Run tests and verify manually**

Run: `cargo make ci`
Expected: All pass.

Manual test (requires running compose stack):
```bash
ur proxy list
ur proxy allow example.com
ur proxy list                # should include example.com
ur proxy block example.com
ur proxy list                # back to original
```

**Step 3: Commit**

```
feat(proxy): runtime proxy domain management via ur CLI

ur proxy {allow,block,list} manages the Squid domain allowlist
directly on the host. Changes written to $UR_CONFIG/squid/allowlist.txt
and applied via docker exec squid -k reconfigure. Intentionally not
exposed via gRPC — agents cannot modify the allowlist.

ur-usca, closes ur-5hsk
```

---

### Task 5: Update Documentation and Tickets

**Files:**
- Modify: `crates/urd/CLAUDE.md`
- Modify: `crates/ur_config/CLAUDE.md`
- Modify: `containers/claude-worker/CLAUDE.md`

**Step 1: Update CLAUDE.md files**

- `crates/urd/CLAUDE.md`: Add bullet about Squid proxy — urd writes config to `$UR_CONFIG/squid/`, signals reconfigure via `docker exec`. Compose manages Squid container lifecycle.
- `crates/ur_config/CLAUDE.md`: Update proxy config section — `hostname` replaces `port`, `SQUID_PORT` constant.
- `containers/claude-worker/CLAUDE.md`: Workers reach proxy at `ur-squid:3128` via Docker DNS.

**Step 2: Update tickets**

```bash
tk close ur-5hsk
tk add-note ur-usca "Replaced custom hyper proxy with Alpine Squid container. pf removed (Docker network isolation via internal network). Runtime allowlist via ur CLI + squid -k reconfigure."
tk add-note ur-3v3e "Compose template must include ur-squid service — see docs/plans/2026-03-09-squid-proxy-ur-usca.md for service definition."
tk add-note ur-jjcf "Build script updated to build ur-squid:latest image."
```

**Step 3: Commit**

```
docs: update CLAUDE.md files for Squid proxy migration

ur-usca
```
