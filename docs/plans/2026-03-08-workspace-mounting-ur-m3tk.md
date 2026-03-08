# Workspace Mounting & tk Script (ur-m3tk)

## Summary

Add `-w`/`--workspace` flag to `ur process launch` to mount a host directory as
the container's `/workspace`. Bundle the `tk` bash script into the container
image. This unblocks milestone 1 by letting humans launch workers against real
repos.

## Changes

### Proto (`core.proto`)

Add `workspace_dir` field to `ProcessLaunchRequest`:

```proto
message ProcessLaunchRequest {
  string process_id = 1;
  string image_id = 2;
  uint32 cpus = 3;
  string memory = 4;
  string workspace_dir = 5;  // absolute host path; empty = legacy git-init behavior
}
```

### CLI (`ur/src/main.rs`)

- Add `-w`/`--workspace` optional `PathBuf` to `ProcessCommands::Launch`
- Resolve to absolute path via `std::fs::canonicalize()`
- Pass as `workspace_dir` in RPC (empty string if not provided)

### Daemon (`urd/src/process.rs`)

**`ProcessManager::prepare()`:**
- If `workspace_dir` is non-empty: skip git-init, skip RepoRegistry registration
- If empty: current behavior

**`ProcessManager::run_and_record()`:**
- Accept `workspace_dir: Option<PathBuf>` in `ProcessConfig`
- If `Some(dir)`: add `(dir, PathBuf::from("/workspace"))` to `RunOpts::volumes`
- If `None`: no volumes (current behavior)

**`ProcessEntry`:**
- Add `externally_managed: bool` — true when `-w` was used
- On stop: skip cleanup for externally-managed workspaces

### Container Image

- Copy `/opt/homebrew/bin/tk` to `containers/claude-worker/tk` (staged build artifact)
- Add `containers/claude-worker/tk` to `.gitignore`
- Add `COPY tk /usr/local/bin/tk` + `chmod +x` to Dockerfile
- Add `Makefile.toml` task `stage-tk` that copies the script

### Git Hooks

No changes needed. The pre-push hook runs on the host. When `-w` mounts the
repo directory, the `.tickets` symlink resolves through the mount to the host's
tickets worktree. `tk` inside the container writes to `.tickets/` which writes
through the mount.

## What This Doesn't Do

- No repo pool management (separate epic)
- No project config in `ur.toml` (separate epic)
- No automatic repo selection by ticket prefix (separate epic)
