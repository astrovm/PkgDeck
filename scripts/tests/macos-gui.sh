#!/usr/bin/env bash
set -euo pipefail
[[ $(uname -s) == Darwin ]] || { echo 'This check requires macOS' >&2; exit 1; }
binary=${1:?Usage: macos-gui.sh /path/to/pkgdeck}
logs="${RUNNER_TEMP:-${TMPDIR:-/tmp}}/pkgdeck-macos-startup"
mkdir -p "$logs"
work=$(mktemp -d)
pid=
# shellcheck disable=SC2329 # Invoked through the EXIT trap.
cleanup() {
    if [[ -n $pid ]]; then
        kill -KILL "$pid" 2>/dev/null || true
        wait "$pid" 2>/dev/null || true
    fi
    rm -rf "$work"
}
trap cleanup EXIT
# Exercise the installed wrapper and native window system outside brew test's
# sandbox. Keep offscreen coverage in the formula and full source GUI suite.
env QT_QPA_PLATFORM=cocoa QT_QUICK_BACKEND=software QT_DEBUG_PLUGINS=1 \
    XDG_CONFIG_HOME="$work" XDG_DATA_HOME="$work" \
    "$binary" --smoke-test >"$logs/cocoa.log" 2>&1 &
pid=$!
for ((second=0; second<60; second++)); do
    if ! kill -0 "$pid" 2>/dev/null; then
        status=0
        wait "$pid" || status=$?
        pid=
        cat "$logs/cocoa.log"
        [[ $status == 0 ]] || exit "$status"
        grep -q PKGDECK_GUI_READY "$logs/cocoa.log"
        exit 0
    fi
    sleep 1
done
# A failed startup must retain the main-thread stack, not only a job timeout.
/usr/bin/sample "$pid" 2 -file "$logs/cocoa.sample.txt" || true
cat "$logs/cocoa.log"
echo 'Installed Cocoa startup did not exit within 60 seconds' >&2
exit 1
