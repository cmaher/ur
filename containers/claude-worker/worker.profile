# ~/.profile: executed by the command interpreter for login shells.

# if running bash and ~/.bashrc exists, source it.
if [ -n "$BASH_VERSION" ]; then
    if [ -f "$HOME/.bashrc" ]; then
        . "$HOME/.bashrc"
    fi
fi

# NOTE: We intentionally do NOT add ~/.local/bin to PATH here.
# PATH is set by ENV in the Dockerfile so /usr/local/bin/claude
# (our wrapper) takes precedence over ~/.local/bin/claude (the
# real binary). Adding ~/.local/bin here would break that ordering
# and also break Claude Code's auto-update.
