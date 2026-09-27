#!/usr/bin/env bash
set -euo pipefail
trap 'echo "native-manager FAILED at line $LINENO: $BASH_COMMAND" >&2' ERR

backend=${1:?Expected dnf, pacman, or zypper}
fallback=true
binary=${2:?Expected absolute pkd binary path}
case "$backend" in
    dnf) image=registry.fedoraproject.org/fedora:44; bootstrap='dnf -y install sudo'; query='rpm -q jq'; manager=dnf ;;
    pacman) image=docker.io/library/archlinux:base; bootstrap='pacman -Sy --noconfirm sudo'; query='pacman -Q jq'; manager=pacman ;;
    zypper) image=registry.opensuse.org/opensuse/tumbleweed:latest; bootstrap='sed -i "s|http://|https://|g" /etc/zypp/repos.d/*.repo; zypper --non-interactive install sudo'; query='rpm -q jq'; manager=zypper
        # download.opensuse.org's redirector can time out for minutes; its
        # regional mirror serves the same repositories.
        fallback='sed -i "s|download.opensuse.org|mirrorcache-us.opensuse.org|g" /etc/zypp/repos.d/*.repo' ;;
    *) echo "Expected dnf, pacman, or zypper" >&2; exit 2 ;;
esac

podman run --rm -v "$binary:/opt/pkd:ro" "$image" bash -lc "
    set -euo pipefail
    trap 'echo \"real $backend lifecycle FAILED at line \$LINENO: \$BASH_COMMAND\" >&2' ERR
    # Distro mirrors time out now and then; setup is not what this tests.
    for attempt in 1 2 3; do
        if ( $bootstrap ); then break; fi
        [[ \$attempt == 3 ]] && exit 1
        $fallback
        sleep 15
    done
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
    if $query; then echo 'Package still installed after removal' >&2; exit 1; fi
    echo 'PASS real $backend lifecycle'
"
