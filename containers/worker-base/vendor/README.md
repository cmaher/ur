# Vendored Dependencies

Files in this directory are vendored copies of upstream installers and tools,
checked into the repository to prevent supply-chain attacks during container
image builds. This avoids fetching scripts from the internet at build time.

To update a vendored file, re-download from its upstream source and commit
the new version.

| Directory     | Source                          | License              |
|---------------|---------------------------------|----------------------|
| `mise/`       | https://mise.run                | MIT (Jeff Dickey)    |
| `superpowers/`| https://github.com/obra/superpowers | See submodule    |

Agent-specific vendored files (e.g. the Claude CLI installer) live in the
corresponding agent directory instead — see `containers/agent-claude/vendor/README.md`.
