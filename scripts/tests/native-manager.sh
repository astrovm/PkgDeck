#!/usr/bin/env bash
set -euo pipefail
trap 'echo "native-manager FAILED at line $LINENO: $BASH_COMMAND" >&2' ERR

backend=${1:?Expected dnf, pacman, or zypper}
binary=${2:?Expected absolute pkd binary path}
case "$backend" in
    dnf) image=registry.fedoraproject.org/fedora:44; bootstrap='dnf -y install sudo'; query='rpm -q jq'; remove='dnf -y remove jq'; manager=dnf ;;
    pacman) image=docker.io/library/archlinux:base; bootstrap='pacman -Sy --noconfirm sudo'; query='pacman -Q jq'; remove='pacman -Rns --noconfirm jq'; manager=pacman ;;
    zypper) image=registry.opensuse.org/opensuse/tumbleweed:latest; bootstrap='zypper --non-interactive install sudo'; query='rpm -q jq'; remove='zypper --non-interactive remove jq'; manager=zypper ;;
    *) echo "Expected dnf, pacman, or zypper" >&2; exit 2 ;;
esac

podman run --rm -v "$binary:/opt/pkd:ro" "$image" bash -lc "
    set -euo pipefail
    trap 'echo \"real $backend lifecycle FAILED at line \$LINENO: \$BASH_COMMAND\" >&2' ERR
    $bootstrap
    useradd -m pkgdeck-test
    printf 'pkgdeck-test ALL=(root) NOPASSWD: /usr/bin/$manager\\n' >/etc/sudoers.d/pkgdeck
    run() { sudo -u pkgdeck-test /opt/pkd --json --yes --auth sudo --from $backend --arch \$(uname -m) \"\$@\"; }
    success() { run \"\$@\" | grep -q '\"exit_code\":0'; }
    success sources
    success search jq
    success info jq
    success update
    success install jq
    $query
    success remove jq
    ! $query
    echo 'PASS real $backend lifecycle'
"
