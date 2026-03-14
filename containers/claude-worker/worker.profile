# ~/.profile: executed by the command interpreter for login shells.

# if running bash and ~/.bashrc exists, source it.
if [ -n "$BASH_VERSION" ]; then
    if [ -f "$HOME/.bashrc" ]; then
        . "$HOME/.bashrc"
    fi
fi

# Restore ~/.local/bin on PATH. Debian's /etc/profile resets PATH for login
# shells, wiping the Docker ENV PATH that included this directory. Claude Code
# and hostexec shims both live in ~/.local/bin, so it must be present.
export PATH="$HOME/.local/bin:$PATH"
