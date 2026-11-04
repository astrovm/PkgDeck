#!/usr/bin/env bash
# Pure askpass protocol tests: no real sudo, passwords or dialogs.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/../.."
work=$(mktemp -d)
trap 'find "$work" -depth -delete' EXIT
mkdir "$work/run"
export HOMEBREW_PKGDECK_ASKPASS_DIR="$work/run"
export ASKPASS_TEST_LOG="$work/dialogs"
export ASKPASS_TEST_RESULT=cancel
cat > "$work/osascript" <<'SCRIPT'
#!/bin/sh
printf 'dialog\n' >> "$ASKPASS_TEST_LOG"
# The script must reject gave-up before reading/printing text returned.
printf '%s\n' "$@" > "$ASKPASS_TEST_LOG.args"
case "$ASKPASS_TEST_RESULT" in
    cancel) exit 1 ;;
    giveup)
        # A real osascript timeout is a successful result record, not an
        # interpreter failure. Replay that record through the real remaining
        # AppleScript, with nonempty text to catch a missing gave-up check.
        if ! grep -q 'giving up after 300' "$ASKPASS_TEST_LOG.args"; then
            printf 'missing timeout\n'
            exit 0
        fi
        answer='set answer to {gave up:true, text returned:"timeout must not leak"}' ;;
    empty) answer='set answer to {gave up:false, text returned:""}' ;;
    ok) answer='set answer to {gave up:false, text returned:"fake fixture password"}' ;;
esac
shift 2
exec /usr/bin/osascript -e "$answer" "$@"
SCRIPT
chmod +x "$work/osascript"
# Replace only the interpreter path in a private copy. Production always uses
# the system interpreter, never an environment-selected password program.
sed "s|/usr/bin/osascript|$work/osascript|g" packaging/macos/pkgdeck-askpass > "$work/askpass"
chmod +x "$work/askpass"
fail() { echo "$*" >&2; exit 1; }
refused() {
    if "$work/askpass" > "$work/output"; then fail 'askpass accepted cancellation'; fi
    [[ ! -s $work/output ]] || fail 'askpass leaked a password on cancellation'
}
for result in cancel giveup empty; do
    export ASKPASS_TEST_RESULT=$result
    rm -f "$work/run/cancelled" "$work/dialogs"
    refused
    refused
    refused
    [[ $(wc -l < "$work/dialogs") -eq 1 ]] || fail "$result prompted again after cancellation"
    [[ -f $work/run/cancelled ]] || fail "$result did not cancel the run"
done
# A later update gets its own directory and asks again. OK remains repeatable.
mkdir "$work/next"
export HOMEBREW_PKGDECK_ASKPASS_DIR="$work/next"
export ASKPASS_TEST_RESULT=ok
for expected in 'fake fixture password' 'fake fixture password'; do
    [[ $("$work/askpass") == "$expected" ]] || fail 'OK password changed'
done
[[ $(wc -l < "$work/dialogs") -eq 3 ]]
[[ ! -e $work/next/cancelled ]]
# The helper also remains usable directly, without a run directory.
unset HOMEBREW_PKGDECK_ASKPASS_DIR
[[ $("$work/askpass") == 'fake fixture password' ]]
export ASKPASS_TEST_RESULT=cancel
refused
echo 'PASS askpass: Cancel, timeout and empty answer prompt once; next run and OK ask again'
