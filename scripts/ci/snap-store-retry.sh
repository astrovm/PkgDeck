#!/usr/bin/env bash
# snapd makes the store requests itself, so curl's CI retry settings do not
# apply. Retry the store timeouts and connection resets seen on hosted runners.
snap_store_retry() {
    local attempt output status
    for attempt in 1 2 3; do
        if output=$("$@" 2>&1); then
            printf '%s\n' "$output"
            return 0
        else
            status=$?
        fi
        printf '%s\n' "$output" >&2
        if (( attempt == 3 )) ||
            ! grep -Eq 'https://api\.snapcraft\.io/.*(Client\.Timeout exceeded while awaiting headers|connection reset by peer)' <<<"$output"; then
            return "$status"
        fi
        printf 'Snap Store connection failed. Retrying attempt %s of 3.\n' "$((attempt + 1))" >&2
        sleep "$((15 * attempt))"
    done
}
