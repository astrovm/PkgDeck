#!/usr/bin/env bash
# AppKit asks IconServices for the generic app icon when a window first
# becomes key, synchronously on the main thread. On a fresh runner the
# daemon can take far longer than a test should wait to answer its first
# request, so CI wakes it at the start of a job and the startup checks wait
# for that answer. A daemon that still hasn't answered is reported as such,
# not as a PkgDeck startup hang.
# Usage: macos-iconservices.sh start
#        macos-iconservices.sh wait SECONDS   (exits 2 if it never answered)
set -euo pipefail
[[ $(uname -s) == Darwin ]] || { echo 'This check requires macOS' >&2; exit 1; }
base="${RUNNER_TEMP:-${TMPDIR:-/tmp}}"
state="$base/pkgdeck-iconservices"
logs="$base/pkgdeck-macos-startup"
mkdir -p "$state" "$logs"

running() {
    [[ -f $state/probe ]] && kill -0 "$(cat "$state/probe")" 2>/dev/null
}

start() {
    if [[ -f $state/answered ]] || running; then
        return 0
    fi
    date +%s > "$state/started"
    # Detached, so it keeps waiting for the daemon after this step ends.
    nohup bash "${BASH_SOURCE[0]}" probe </dev/null >/dev/null 2>&1 &
    disown
    for ((tries=0; tries<50; tries++)); do
        [[ -f $state/probe ]] && break
        sleep 0.1
    done
}

# Asks for an icon, and notes when and how the answer came.
probe() {
    local status=0
    /usr/bin/osascript -l JavaScript \
        -e 'ObjC.import("AppKit"); $.NSWorkspace.sharedWorkspace.iconForFile("/Applications"); "ok"' \
        >"$logs/iconservices.log" 2>&1 &
    echo $! > "$state/probe"
    wait $! || status=$?
    echo "$status $(( $(date +%s) - $(cat "$state/started") ))" > "$state/answered"
}

wait_for_answer() {
    local limit=$1 second waited status took
    start
    for ((second=0; second<limit; second++)); do
        [[ -f $state/answered ]] && break
        sleep 1
    done
    if [[ ! -f $state/answered ]]; then
        waited=$(( $(date +%s) - $(cat "$state/started") ))
        if running; then
            /usr/bin/sample "$(cat "$state/probe")" 2 -file "$logs/iconservices.sample.txt" || true
        fi
        echo "::warning::IconServices did not answer within $waited seconds; skipping the Cocoa startup check on this runner"
        return 2
    fi
    read -r status took < "$state/answered"
    echo "IconServices answered $took seconds after it was first asked"
    if [[ $status != 0 ]]; then
        cat "$logs/iconservices.log"
        echo 'IconServices probe failed' >&2
        return 1
    fi
}

case "${1:-}" in
    start) start ;;
    probe) probe ;;
    wait) wait_for_answer "${2:?Usage: macos-iconservices.sh wait SECONDS}" ;;
    *) echo 'Usage: macos-iconservices.sh start|wait SECONDS' >&2; exit 2 ;;
esac
