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
# Qt asks IconServices for the generic app icon when a window first becomes
# key, synchronously on the main thread. Some runners start iconservicesd
# lazily or leave it unresponsive; probe it from a separate process so a
# stalled daemon is reported as such instead of as a PkgDeck startup hang.
/usr/bin/osascript -l JavaScript \
    -e 'ObjC.import("AppKit"); $.NSWorkspace.sharedWorkspace.iconForFile("/Applications"); "ok"' \
    >"$logs/iconservices.log" 2>&1 &
probe=$!
for ((second=0; second<120; second++)); do
    kill -0 "$probe" 2>/dev/null || break
    sleep 1
done
if kill -0 "$probe" 2>/dev/null; then
    /usr/bin/sample "$probe" 2 -file "$logs/iconservices.sample.txt" || true
    kill -KILL "$probe" 2>/dev/null || true
    wait "$probe" 2>/dev/null || true
    echo '::warning::IconServices did not answer within 120 seconds; skipping the Cocoa startup check on this runner'
    exit 0
fi
wait "$probe" || { cat "$logs/iconservices.log"; echo 'IconServices probe failed' >&2; exit 1; }
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
