# tmux

Typed Rust interface over the tmux CLI. Used by workerd (session creation, send-keys) and
the host CLI (attach command generation).

- `Session` is the primary type — created via `Session::create()`, then used for `send_keys()`, `set_status_left()`, etc.
- `send_keys()` sends text via `send-keys -l` (literal mode — the argument is text, not key names)
- `send_keys_no_enter()` types text without submitting; `send_enter()` presses Enter on its own
- `send_keys_raw()` bypasses `-l` for control sequences like `Enter`, `C-c`
- `attach_command()` returns command parts for use with container runtime `exec_interactive`
- All async operations use `tokio::process::Command` under the hood

## Submitting to Claude Code (`send_keys`)

Claude Code does **not** reliably submit when the text and the Enter arrive back-to-back —
the Enter gets absorbed by the still-rendering input box and the command sits there
unsubmitted. `send_keys()` therefore does more than type-and-Enter:

1. Clears the input box first if it is non-empty (`C-c`, Claude Code's clear-input binding),
   so text is never appended to whatever was already there. Sent at most once — a second
   `C-c` on an empty box starts Claude Code's exit confirmation.
2. Waits `SUBMIT_SETTLE` (400ms) between the text and the Enter.
3. Confirms the box drained and re-presses Enter up to `SUBMIT_RETRIES` times if not,
   erroring if the text is still there afterwards.

A full `send_keys()` therefore takes ~900ms. Steps 1 and 3 read Claude Code's input box out
of the pane (the last line starting with `❯` — earlier ones are transcript echoes of
already-submitted messages). Panes not running Claude Code have no such line, so those steps
no-op and the call degrades to plain type-then-Enter.

Callers must not send while Claude Code is *busy*: input that arrives mid-turn lands in its
queued-message buffer instead of executing, and a queued slash command never runs. See
workerd's `spawn_deferred_send`.
