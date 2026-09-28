#!/usr/bin/env bash
# Install MacPorts from its official package on a disposable Mac, then drive a
# real port through pkd. State is confirmed with `port` itself, never only
# PkgDeck output. Writes go through `sudo -n` (pkd --auth sudo).
# Usage: scripts/tests/macports-manager.sh [pkd]
set -Eeuo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/../.."
[[ ${GITHUB_ACTIONS:-} == true && ${GITHUB_REPOSITORY:-} == astrovm/PkgDeck &&
   ${RUNNER_ENVIRONMENT:-} == github-hosted && ${RUNNER_OS:-} == macOS &&
   $EUID != 0 && -n ${GITHUB_WORKSPACE:-} &&
   $(cd "$GITHUB_WORKSPACE" && pwd -P) == "$(pwd -P)" ]] || {
    echo 'Requires the disposable PkgDeck GitHub-hosted macOS runner.' >&2
    exit 1
}
pkd=${1:-$PWD/target/debug/pkd}
logs="$PWD/build/macports"
mkdir -p "$logs"
exec > >(tee "$logs/lifecycle.log") 2>&1
trap 'echo "macports-manager FAILED at line $LINENO: $BASH_COMMAND" >&2' ERR
[[ -x $pkd ]]
command -v jq
sudo -n true
sw_vers
[[ ! -e /opt/local ]] || { echo 'MacPorts is already installed; refusing to reuse it.' >&2; exit 1; }

# Kill a step that outlives its budget instead of waiting for the job timeout.
bounded() {
    local seconds=$1 pid watchdog status=0
    shift
    "$@" &
    pid=$!
    (sleep "$seconds" && echo "Timed out after ${seconds}s: $*" >&2 && sudo -n kill -TERM "$pid" 2>/dev/null) &
    watchdog=$!
    wait "$pid" || status=$?
    kill "$watchdog" 2>/dev/null || true
    return "$status"
}

# Pinned release; the checksums come from its MacPorts-2.12.6.chk.txt.
release=2.12.6
case "$(sw_vers -productVersion | cut -d. -f1)" in
26) flavor=26-Tahoe sha256=ddd90723ba470a688296bb520335e1c7c08835d82df4141e6807b411bc8b78e8 ;;
27) flavor=27-GoldenGate sha256=f6f6508bd3ecc54382fbc479673fee0dbb9b63b708a64dee9d7479cd4b3ca918 ;;
*) echo "No pinned MacPorts $release package for macOS $(sw_vers -productVersion)" >&2; exit 1 ;;
esac
installer_pkg="$RUNNER_TEMP/MacPorts-$release-$flavor.pkg"
curl -fsSL --retry 3 -o "$installer_pkg" \
    "https://github.com/macports/macports-base/releases/download/v$release/MacPorts-$release-$flavor.pkg"
echo "$sha256  $installer_pkg" | shasum -a 256 -c -
# The package's postflight already runs `port selfupdate`.
bounded 900 sudo -n installer -pkg "$installer_pkg" -target /
# A shell profile puts MacPorts on PATH; pkd reads through that PATH and
# writes only through the fixed /opt/local/bin/port.
export PATH="/opt/local/bin:/opt/local/sbin:$PATH"
port version

run() { "$pkd" --json --yes --auth sudo --from macports "$@"; }
success() {
    local output start=$SECONDS
    output=$(run "$@") || true
    if ! grep -q '"exit_code":0' <<<"$output"; then
        echo "pkd $* failed after $((SECONDS - start))s: $output" >&2
        return 1
    fi
}
# `port installed` may exit non-zero when nothing matches.
installed() { port -q installed || true; }
absent() {
    if grep "$1" >/dev/null; then
        echo "Port still present after removal: $1" >&2
        return 1
    fi
}
# Every active port, as `port` reports it and as pkd lists it.
agree() {
    local expected actual
    expected=$(installed | awk '/\(active\)/ { sub(/^@/, "", $2); print $1, $2 }' | sort)
    actual=$(run list | jq -r '.data.packages[] | "\(.id.name) \(.installed_version)"' | sort)
    if [[ $expected != "$actual" ]]; then
        printf 'port and pkd disagree.\nport:\n%s\npkd:\n%s\n' "$expected" "$actual" >&2
        return 1
    fi
}

success sources
run sources | jq -e '.data.sources[] | select(.backend == "macports") | .availability.Ok == "available"'
bounded 900 success refresh
# ripgrep has a binary archive and a default variant (+pcre) whose pcre2
# dependency comes along.
installed | absent '^ *ripgrep '
run search ripgrep > "$logs/search.json"
jq -e '.data.packages[] | select(.id.name == "ripgrep") | .installed_version == null' "$logs/search.json"
bounded 1200 success install ripgrep
port -q installed ripgrep | grep -E '^ *ripgrep @[^ ]+\+pcre.* \(active\)'
/opt/local/bin/rg --version
agree
run list > "$logs/installed.json"
jq -e '.data.packages[] | select(.id.name == "ripgrep") | (.installed_version | test("^[0-9][^ ]*_[0-9]+\\+pcre")) and .update == "current"' "$logs/installed.json"
run info ripgrep > "$logs/details.json"
jq -e '.data.package | .id.scope == "system" and (.installed_version | contains("+pcre"))' "$logs/details.json"
before=$(port -q installed ripgrep)
bounded 900 success upgrade ripgrep
[[ $(port -q installed ripgrep) == "$before" ]]
bounded 600 success remove ripgrep
installed | absent '^ *ripgrep '
[[ ! -e /opt/local/bin/rg ]]
agree
# Removing it again fails instead of reporting success.
output=$(run remove ripgrep) || true
if grep -q '"exit_code":0' <<<"$output"; then
    echo "Removing an absent port succeeded: $output" >&2
    exit 1
fi
echo 'PASS MacPorts install, variants, upgrade, removal and inventory through pkd'
