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
for ((second=0; second<20; second++)); do
    kill -0 "$probe" 2>/dev/null || break
    sleep 1
done
if kill -0 "$probe" 2>/dev/null; then
    /usr/bin/sample "$probe" 2 -file "$logs/iconservices.sample.txt" || true
    kill -KILL "$probe" 2>/dev/null || true
    wait "$probe" 2>/dev/null || true
    echo '::warning::IconServices did not answer within 20 seconds; skipping the Cocoa startup check on this runner'
    exit 0
fi
wait "$probe" || { cat "$logs/iconservices.log"; echo 'IconServices probe failed' >&2; exit 1; }
# Print the crashed thread of the report macOS writes for a run that died
# from a signal; ReportCrash can take a few seconds to write it.
crash_report() {
    local run=$1 report= second
    for ((second=0; second<30; second++)); do
        report=$(find "$HOME/Library/Logs/DiagnosticReports" -maxdepth 1 -iname 'pkgdeck*.ips' \
            -newer "$logs/cocoa-$run.log" 2>/dev/null | head -n 1)
        [[ -z $report ]] || break
        sleep 1
    done
    if [[ -z $report ]]; then
        echo "No crash report was written for run $run"
        return
    fi
    cp "$report" "$logs/cocoa-$run.ips"
    echo "Crash report for run $run: $(basename "$report")"
    # The report is a JSON header line followed by the JSON body.
    tail -n +2 "$report" | jq -r '
        . as $report
        | "exception: \(.exception | tostring)",
          "termination: \(.termination | tostring)",
          "asi: \(.asi // {} | tostring)",
          (.threads[] | select(.triggered) | "crashed thread: \(.name // .queue // "unnamed")",
            (.frames[] | "  \($report.usedImages[.imageIndex].name // "?")  \(.symbol // "?")+\(.symbolLocation // 0)"))
    ' || head -c 20000 "$report"
}
# Exercise the installed wrapper and native window system outside brew test's
# sandbox. Keep offscreen coverage in the formula and full source GUI suite.
# Shutdown has crashed intermittently, so one clean run is not enough.
runs=15
for ((run=1; run<=runs; run++)); do
    env QT_QPA_PLATFORM=cocoa QT_QUICK_BACKEND=software QT_DEBUG_PLUGINS=1 \
        XDG_CONFIG_HOME="$work" XDG_DATA_HOME="$work" \
        "$binary" --smoke-test >"$logs/cocoa-$run.log" 2>&1 &
    pid=$!
    status=
    # The first launch of a freshly installed app waits on macOS services
    # (registration, security checks) that a busy runner can take minutes
    # to answer; later launches must start promptly.
    limit=60
    ((run == 1)) && limit=180
    for ((second=0; second<limit; second++)); do
        if ! kill -0 "$pid" 2>/dev/null; then
            status=0
            wait "$pid" || status=$?
            pid=
            break
        fi
        sleep 1
    done
    if [[ -z $status ]]; then
        # A failed startup must retain the main-thread stack, not only a job timeout.
        /usr/bin/sample "$pid" 2 -file "$logs/cocoa-$run.sample.txt" || true
        cat "$logs/cocoa-$run.log"
        # Print the stacks here too: a failed step can end the job before
        # its diagnostics are uploaded.
        sed -n '/^Call graph:/,/^Total number in stack/p' "$logs/cocoa-$run.sample.txt" 2>/dev/null | head -n 200 || true
        echo "Installed Cocoa startup did not exit within $limit seconds (run $run of $runs)" >&2
        exit 1
    fi
    if [[ $status != 0 ]]; then
        cat "$logs/cocoa-$run.log"
        crash_report "$run"
        echo "Installed Cocoa startup exited with status $status (run $run of $runs)" >&2
        exit "$status"
    fi
    grep -q PKGDECK_GUI_READY "$logs/cocoa-$run.log" || {
        cat "$logs/cocoa-$run.log"
        echo "Installed Cocoa startup never became ready (run $run of $runs)" >&2
        exit 1
    }
    # The pipe is a literal separator. grep without -F would treat it as OR.
    grep -qF 'PKGDECK_TRAY_MENU Open|Check now|Quit' "$logs/cocoa-$run.log" || {
        cat "$logs/cocoa-$run.log"
        echo "Installed Cocoa startup did not open the menu bar menu (run $run of $runs)" >&2
        exit 1
    }
done
echo "Installed Cocoa startup passed $runs runs"
