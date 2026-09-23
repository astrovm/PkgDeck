#!/usr/bin/env bash
set -euo pipefail
trap 'echo "guest FAILED at line $LINENO: $BASH_COMMAND" >&2' ERR
[[ ${1:-} == --container && $EUID == 0 && -f /etc/pkgdeck-prepared &&
    -f /run/.containerenv && -f /etc/pkgdeck-disposable-container ]]
# Build the helper against this guest's APT ABI; frontends remain the host-built binaries.
mkdir -p /opt/pkgdeck-bin
cp /mnt/pkgdeck-bin/pkd /opt/pkgdeck-bin/
cd /mnt/pkgdeck
scripts/build-apt.sh /opt/pkgdeck-bin
useradd -m pkgdeck-test
printf 'pkgdeck-test ALL=(root) NOPASSWD: /usr/bin/apt-get\n' >/etc/sudoers.d/pkgdeck-fixture
chmod 440 /etc/sudoers.d/pkgdeck-fixture
source scripts/vm/lifecycle.sh
apt_fixture
apt_lifecycle
brew_lifecycle
echo PKGDECK_CONTAINER_PASS
