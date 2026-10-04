#!/usr/bin/env bash
# Homebrew bootstraps RuboCop from RubyGems before checking style. Retry only
# bootstrap/network failures; an actual style violation fails immediately.
set -euo pipefail
log=$(mktemp)
trap 'rm -f "$log"' EXIT
for attempt in 1 2 3; do
    if brew style "$@" 2>&1 | tee "$log"; then
        exit 0
    else
        status=$?
    fi
    if [[ $attempt == 3 ]] || ! grep -Eq 'Network error while fetching|Failed to open TCP connection|Error: failed to run .*bundle install' "$log"; then
        exit "$status"
    fi
    echo "Homebrew style setup failed; retrying ($attempt/3)." >&2
    sleep 5
done
