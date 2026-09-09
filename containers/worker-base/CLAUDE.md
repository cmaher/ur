# worker-base (Container Image)

Debian bookworm-slim base image, agent-agnostic. Must work with Docker and nerdctl (containerd) runtimes.

- Build context is `containers/worker-base/` — all files copied into the image must live here
- Image is tagged `ur-worker-base:latest` by convention
- Installs shell tooling (tmux, jq, yq, ripgrep, etc.), locale, and a non-root `worker` user (Claude Code and other agents refuse `--dangerously-skip-permissions` as root)
- Bakes in a clean shell profile (`worker.profile`, `worker.bashrc`) — Debian's default `~/.profile` modifies `PATH` in ways that can conflict with the image's `ENV PATH` ordering
- Ships agent-agnostic content under `/home/worker/.agent-shared/`: `potential-skills/` (skills every agent can install), `instructions/` (flat `{strategy}.md` files — code/design/manual), and `shared-instructions/` (fragments appended to every strategy file). Agent images copy the pieces they need into their own home-subdir layout at init time (e.g. workerd's `InitSkillsManager`/`InitInstructionsManager` for Claude)
- `yq` is Debian's package (the Python jq wrapper, which also provides `xq`/`tomlq`) — **not** mikefarah's Go binary. Queries use jq syntax over YAML (`yq '.a.b' f.yaml`); the Go-only forms (`yq -i '.a = "b"' f.yaml`, `yq e`, `yq eval`) are not available. For YAML output pass `-y`
- **No agent CLI is installed here.** Agent-specific installs (e.g. the Claude Code CLI), app config, and the worker binaries live in the agent layer built on top of this one — see `containers/worker-claude/`
