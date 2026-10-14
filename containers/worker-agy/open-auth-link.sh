#!/bin/bash
set -euo pipefail

# AGY's login renderer hard-wraps the URL without OSC 8 metadata. Give it a
# browser opener that displays a short hyperlink in a separate tmux pane.
if [[ "${1:-}" == --display ]]; then
    url="$2"
    printf '\033]8;;%s\033\\Open AGY sign-in\033]8;;\033\\\n' "$url"
    printf 'Cmd-click the link above. Close this pane after signing in.\n'
    exec sleep infinity
fi

url="${1:?missing browser URL}"
if [[ "$url" != https://* || "$url" == *$'\n'* || "$url" == *$'\r'* || "$url" == *$'\e'* || "$url" == *$'\a'* ]]; then
    printf 'Expected an HTTPS browser URL without control characters\n' >&2
    exit 1
fi

if [[ -n "${TMUX_PANE:-}" ]]; then
    tmux split-window -d -v -l 3 -t "$TMUX_PANE" "$0" --display "$url"
else
    printf '\033]8;;%s\033\\Open AGY sign-in\033]8;;\033\\\n' "$url"
fi
