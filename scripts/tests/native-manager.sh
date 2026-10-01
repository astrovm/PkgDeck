#!/usr/bin/env bash
# Real install/remove lifecycle through a distro package manager in a container.
# apk (musl) and xbps need a static musl pkd; the others take the glibc build.
set -euo pipefail
trap 'echo "native-manager FAILED at line $LINENO: $BASH_COMMAND" >&2' ERR

backend=${1:?Expected dnf, pacman, zypper, apk, or xbps}
binary=${2:?Expected absolute pkd binary path}
engine=${PKGDECK_CONTAINER_ENGINE:-podman}
adduser='useradd -m pkgdeck-test'
fallback=true
case "$backend" in
    dnf) image=registry.fedoraproject.org/fedora:44; bootstrap='dnf -y install sudo'; query='rpm -q jq'; writers=/usr/bin/dnf ;;
    pacman) image=docker.io/library/archlinux:base; bootstrap='pacman -Sy --noconfirm sudo'; query='pacman -Q jq'; writers=/usr/bin/pacman ;;
    zypper) image=registry.opensuse.org/opensuse/tumbleweed:latest; bootstrap='sed -i "s|http://|https://|g" /etc/zypp/repos.d/*.repo; zypper --non-interactive install sudo'; query='rpm -q jq'; writers=/usr/bin/zypper
        # download.opensuse.org's redirector can time out for minutes; its
        # regional mirror serves the same repositories.
        fallback='sed -i "s|download.opensuse.org|mirrorcache-us.opensuse.org|g" /etc/zypp/repos.d/*.repo' ;;
    apk) image=docker.io/library/alpine:latest; bootstrap='apk add bash sudo'; query='apk info -e jq'; writers=/sbin/apk; adduser='adduser -D pkgdeck-test' ;;
    # XBPS must update itself before it installs anything else. The image has
    # no /etc/shadow, which sudo's PAM account check needs.
    xbps) image=ghcr.io/void-linux/void-glibc:latest; bootstrap='xbps-install -Syu xbps && xbps-install -yu && xbps-install -y bash sudo shadow'; query='xbps-query jq >/dev/null'; writers='/usr/bin/xbps-install, /usr/bin/xbps-remove'; adduser='pwconv && useradd -m pkgdeck-test' ;;
    *) echo "Expected dnf, pacman, zypper, apk, or xbps" >&2; exit 2 ;;
esac

script="
    set -Eeuo pipefail
    trap 'echo \"real $backend lifecycle FAILED at line \$LINENO: \$BASH_COMMAND\" >&2' ERR
    $adduser
    printf 'pkgdeck-test ALL=(root) NOPASSWD: $writers\\n' >/etc/sudoers.d/pkgdeck
    run() { sudo -u pkgdeck-test /opt/pkd --json --yes --auth sudo --from $backend --arch \$(uname -m) \"\$@\"; }
    # Print pkd's report when a step fails; grep -q alone would hide it.
    success() { local out; out=\$(run \"\$@\") || true; grep -q '\"exit_code\":0' <<<\"\$out\" || { printf '%s\\n' \"\$out\" >&2; return 1; }; }
    success sources
    success search jq
    success info jq
    success update
    success install jq
    $query
    run list | grep -q '\"name\":\"jq\"'
    success remove jq
    if $query; then echo 'Package still installed after removal' >&2; exit 1; fi
    echo 'PASS real $backend lifecycle'
"
# Alpine and Void ship without bash, so the bootstrap runs in POSIX sh; it is
# single-quoted on purpose so it expands inside the container.
# shellcheck disable=SC2016
"$engine" run --rm -v "$binary:/opt/pkd:ro" "$image" sh -c '
    # Distro mirrors time out now and then; setup is not what this tests.
    for attempt in 1 2 3; do
        if (eval "$1"); then break; fi
        [ "$attempt" = 3 ] && exit 1
        eval "$3"
        sleep 15
    done
    exec bash -lc "$2"
' sh "$bootstrap" "$script" "$fallback"
