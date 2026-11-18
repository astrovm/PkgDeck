#!/usr/bin/env bash
# snapd makes the store requests itself, so curl's CI retry settings do not
# apply. Retry only the store timeouts seen on hosted runners.
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
        if (( attempt == 3 )) || [[ $output != *'https://api.snapcraft.io/'* ||
                                   $output != *'Client.Timeout exceeded while awaiting headers'* ]]; then
            return "$status"
        fi
        printf 'Snap Store timed out. Retrying attempt %s of 3.\n' "$((attempt + 1))" >&2
        sleep "$((15 * attempt))"
    done
}
