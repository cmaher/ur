# Vendored Dependencies

Files in this directory are vendored copies of upstream installers,
checked into the repository to prevent supply-chain attacks during container
image builds. This avoids fetching scripts from the internet at build time.

To update a vendored file, re-download from its upstream source and commit
the new version.

| Directory     | Source                                          | License          |
|---------------|--------------------------------------------------|------------------|
| `codex/`      | https://github.com/openai/codex                  | See upstream repository |

Unlike `vendor/claude/install.sh`, `codex/install.sh` still resolves the
release *metadata* (asset URL for the current version) from GitHub's API at
build time — the script itself is vendored, but codex has no fixed-checksum
manifest to pin against the way Claude Code's GCS bucket does.

Agent-agnostic vendored files (mise, superpowers) live in
`containers/worker-base/vendor/` instead.
