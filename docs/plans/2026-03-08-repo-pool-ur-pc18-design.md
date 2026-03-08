# Repo Pool Management Design (ur-pc18)

## Summary

Urd manages a pool of pre-cloned repos per project. Projects are configured in
`ur.toml` with a ticket prefix and git remote URL. When `ur process launch`
runs without `-w`, urd finds an available clone from the pool (or creates one)
and mounts it. On process stop, the clone is reset and returned to the pool.

## Project Config

```toml
[[projects]]
prefix = "ur"
repo = "git@github.com:cmaher/ur.git"
# name defaults to "ur" (derived from repo URL, last path segment minus .git)

[[projects]]
prefix = "swa"
repo = "git@github.com:cmaher/swa.git"
name = "swa-custom-name"  # optional override
```

## Pool Mechanics

**Storage:** `$WORKSPACE/pool/<project-name>/<slot-index>/` (e.g.
`~/.ur/workspace/pool/ur/0/`, `.../1/`, etc.)

**Acquire (on process launch without `-w`):**
1. Parse ticket prefix from `process_id` to find project
2. Scan pool slots for one not in use by a running process
3. If found: mark as in-use, mount it
4. If none available: `git clone <repo_url>` into a new slot, mark as in-use, mount it

**Release (on process stop):**
1. Run `git fetch origin && git checkout master && git reset --hard origin/master && git clean -fd`
2. Mark slot as available

**In-memory tracking (temporary):** HashMap of slot paths → process_id (or None).
Will move to CozoDB with ur-o79g.

## Ticket Prefix Resolution

Given `process_id = "ur-abc12"`, extract prefix `"ur"`, look up in projects
config. Error if no matching project and no `-w` flag.

## Dependencies

- ur-m3tk workspace mounting (volume mount plumbing)
