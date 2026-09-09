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
build time: the script itself is vendored and reviewable, but codex publishes
no fixed-version manifest to pin a URL against the way Claude Code's GCS
bucket does, so "latest release" is resolved per build.

The payload is not trusted blindly. `codex/install.sh` matches the exact
expected asset name, then verifies the downloaded archive against that
release's own `<asset>.sha256` sidecar before installing it; a mismatch fails
the build. A missing or unparseable sidecar warns and proceeds, since upstream
owns that layout — if you see that warning, check whether the sidecars moved
before shipping the image.

Agent-agnostic vendored files (mise, superpowers) live in
`containers/worker-base/vendor/` instead.
