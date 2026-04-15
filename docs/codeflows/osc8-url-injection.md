# OSC 8 URL Injection (worker tmux ctrl+click)

## Problem

Inside a worker's tmux session, a long URL printed by Claude Code wraps onto
multiple visual rows. tmux stores each row independently in its grid and
re-emits each as a discrete hard line to the outer terminal, so WezTerm /
iTerm2 see two unrelated strings and ctrl+click follows only the first
fragment. Outside tmux, the same URL is one auto-wrapped logical line and
ctrl+click works.

## Fix

Insert a transparent PTY filter between Claude Code and tmux that wraps every
detected URL in OSC 8 hyperlink escapes. tmux 3.4+ stores the hyperlink ID
per cell and forwards it to the outer terminal regardless of visual wrap, so
the full URL stays clickable.

```
Claude Code → ur-osc8 PTY filter → tmux pane grid → outer terminal
                  ↑
            injects OSC 8
```

## Components

- `crates/ur-osc8/src/inject.rs` — pure ANSI-aware streaming `Injector`.
  Tracks parser state across chunks (in-text / in-CSI / in-OSC / in-ESC,
  inside-OSC-8 flag, bounded ≤4 KiB pending buffer for partial URLs at
  chunk boundaries). Aborts an in-progress URL on CSI cursor moves. Skips
  injection inside existing OSC 8 spans and inside CSI/OSC payloads.
- `crates/ur-osc8/src/main.rs` — `portable-pty` binary. Spawns the child on
  a PTY, pumps child output through `Injector` to stdout, forwards stdin
  and SIGWINCH, propagates the child's exit code.
- `containers/claude-worker/.tmux.conf` — `set -ga terminal-features
  '*:hyperlinks'` so tmux treats the outer terminal as hyperlink-capable
  regardless of `$TERM`.
- `crates/workerd/src/main.rs` — launches Claude as `ur-osc8 -- claude`.
- `containers/claude-worker/stage-workercmd.sh` + `Dockerfile` — cross-compile
  and stage `/usr/local/bin/ur-osc8` into the worker image.

## Manual acceptance

Automated UI testing of ctrl+click is not feasible. To validate end-to-end:

1. Build and launch a worker with the new image.
2. Have Claude print a URL longer than the pane width, e.g.
   `echo https://example.com/some/very/long/path/that/will/definitely/wrap/across/two/visual/rows`.
3. **WezTerm**: ctrl+click the URL — full URL should open, not a fragment.
4. **iTerm2**: ctrl+click the URL — full URL should open.
5. **Control (no tmux)**: same URL printed in a non-tmux terminal still
   ctrl+clickable (no regression).
6. Mouse-drag copy of the URL is unchanged (out of scope; verify only that
   nothing got worse).

## Failure modes

- Injector panic: `main.rs` wraps each chunk in `catch_unwind` and falls
  back to raw passthrough plus a stderr message instead of killing Claude
  silently.
- `ur-osc8 -- claude` is functionally identical to `claude` for I/O and
  exit code; only OSC 8 escapes are added.
