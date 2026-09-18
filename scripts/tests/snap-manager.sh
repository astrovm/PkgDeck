#!/usr/bin/env bash
# Exercise the real Snap backend through pkd against snapd.
# Requires a systemd host with snapd running (Ubuntu runners provide it) and
# sudo once to create the unprivileged test user and its snap-only grant.
# Lifecycle state is confirmed through snap itself, never only PkgDeck output.
# Usage: scripts/tests/snap-manager.sh <pkd>
set -euo pipefail
trap 'echo "snap-manager FAILED at line $LINENO: $BASH_COMMAND" >&2' ERR
pkd=${1:?Usage: scripts/tests/snap-manager.sh <pkd>}
command -v snap >/dev/null || { echo 'snapd is required' >&2; exit 1; }
echo "snap-manager: pkd=$pkd user=$(whoami)"
install -m 755 "$pkd" /tmp/pkgdeck-pkd
if ! id pkgdeck-test >/dev/null 2>&1; then sudo useradd -m pkgdeck-test; fi
echo 'pkgdeck-test ALL=(root) NOPASSWD: /usr/bin/snap' | sudo tee /etc/sudoers.d/pkgdeck-snap >/dev/null
run() { sudo -u pkgdeck-test /tmp/pkgdeck-pkd --json --yes --auth sudo --from snap "$@"; }
success() { run "$@" | grep -q '"exit_code":0'; }
success sources
success search hello-world
success info hello-world
success update
success install hello-world
snap list hello-world
success remove hello-world
! snap list hello-world
echo 'PASS real snap lifecycle'
