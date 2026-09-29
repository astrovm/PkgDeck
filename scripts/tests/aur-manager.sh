#!/usr/bin/env bash
# The AUR source against real foreign packages on Arch Linux: listing,
# update checks against the real AUR, an update through the AUR helper, and
# a removal through Pacman.
# No AUR PKGBUILD is ever downloaded or run: the fixtures are empty packages
# built here, two of them under real AUR names so the AUR RPC has versions
# to compare against, and the helper is a stand-in that builds another one.
# The container script is single-quoted on purpose: it expands inside it.
# shellcheck disable=SC2016
set -euo pipefail
trap 'echo "aur-manager FAILED at line $LINENO: $BASH_COMMAND" >&2' ERR

binary=${1:?Expected absolute pkd binary path}
engine=${PKGDECK_CONTAINER_ENGINE:-podman}

script='
    set -Eeuo pipefail
    trap '\''echo "real aur lifecycle FAILED at line $LINENO: $BASH_COMMAND" >&2'\'' ERR
    useradd -m pkgdeck-test
    # makepkg refuses root, so the fixtures are built as the test user.
    # Empty packages: fixture is in no repository and not in the AUR; yay
    # is older and paru newer than what the AUR has.
    fixture() {
        local dir=/home/pkgdeck-test/$1
        mkdir -p "$dir"
        printf "%s\n" "pkgname=$1" "pkgver=$2" "pkgrel=1" "pkgdesc=\"PkgDeck AUR test fixture\"" \
            "arch=(any)" "license=(MIT)" "options=(!strip !debug)" "package() { :; }" >"$dir/PKGBUILD"
        chown -R pkgdeck-test: "$dir"
        (cd "$dir" && sudo -u pkgdeck-test makepkg --nodeps --noconfirm >/dev/null)
        pacman -U --noconfirm "$dir"/*.pkg.tar.* >/dev/null
    }
    fixture pkgdeck-fixture-foreign 1.0
    fixture yay 0.1
    fixture paru 999
    run() { sudo -u pkgdeck-test /opt/pkd --json --from "$@"; }
    row() { jq -ec --arg name "$1" '\''.data.packages[] | select(.id.name == $name)'\''; }

    # The AUR RPC drops connections now and then. A command that still fails
    # prints its report before the assertion stops the test.
    ok() {
        local out attempt
        for attempt in 1 2 3; do
            out=$(run "$@" || true)
            if jq -e ".exit_code == 0" <<<"$out" >/dev/null; then echo "$out"; return; fi
            [[ $attempt == 3 ]] || sleep 10
        done
        echo "$out" >&2
        return 1
    }
    aur=$(ok aur list)
    row pkgdeck-fixture-foreign <<<"$aur" | jq -e '\''.installed_version == "1.0-1" and .update == "current" and (.summary | startswith("Not in the AUR"))'\'' >/dev/null
    row yay <<<"$aur" | jq -e '\''.installed_version == "0.1-1" and .update == "available" and .candidate_version != null'\'' >/dev/null
    row paru <<<"$aur" | jq -e '\''.installed_version == "999-1" and .update == "current"'\'' >/dev/null
    # Repository packages stay under Pacman, foreign ones only under the AUR.
    if row pacman <<<"$aur" >/dev/null; then echo "Repository package listed under the AUR" >&2; exit 1; fi
    pacman=$(ok pacman list)
    row pacman <<<"$pacman" >/dev/null
    for name in pkgdeck-fixture-foreign yay paru; do
        if row "$name" <<<"$pacman" >/dev/null; then echo "$name listed under Pacman" >&2; exit 1; fi
    done

    ok aur search fixture | row pkgdeck-fixture-foreign >/dev/null
    ok aur info yay | jq -e "tostring | contains(\"AUR version\")" >/dev/null

    # Installing stays with the AUR helper: it would build a PKGBUILD.
    out=$(run aur --yes install yay || true)
    jq -e ".exit_code != 0" <<<"$out" >/dev/null || { echo "AUR install was not refused: $out" >&2; exit 1; }
    # Without a helper there is nothing to build with, and nothing changes.
    out=$(run aur --yes upgrade yay || true)
    grep -q "install paru or yay" <<<"$out" || { echo "Expected a missing-helper error: $out" >&2; exit 1; }
    [[ $(pacman -Q yay) == "yay 0.1-1" ]]

    # A stand-in paru ends the way the real helper does: it installs the
    # built package with sudo, here an empty yay at the AUR version.
    row yay <<<"$aur" | jq -r .candidate_version >/etc/pkgdeck-aur-latest
    echo "pkgdeck-test ALL=(root) NOPASSWD: /usr/bin/pacman" >/etc/sudoers.d/pkgdeck
    cat >/usr/local/bin/paru <<"STANDIN"
#!/bin/bash
set -euo pipefail
if [[ ${1:-} == --version ]]; then echo "paru v2.1.0 (PkgDeck stand-in)"; exit 0; fi
echo "$*" >/tmp/paru-args
latest=$(cat /etc/pkgdeck-aur-latest)
dir=$(mktemp -d)
cd "$dir"
printf "%s\n" "pkgname=${!#}" "pkgver=${latest%-*}" "pkgrel=${latest##*-}" "arch=(any)" \
    "license=(MIT)" "options=(!strip !debug)" "package() { :; }" >PKGBUILD
makepkg --nodeps --noconfirm >/dev/null
sudo -n pacman -U --noconfirm ./*.pkg.tar.*
STANDIN
    chmod 755 /usr/local/bin/paru
    ok aur --yes upgrade yay >/dev/null
    # pkd --auth sudo never prompts: the helper was told to use sudo -n.
    grep -q -- "--needed --noconfirm --skipreview --sudoflags=-n -- yay" /tmp/paru-args
    [[ $(pacman -Q yay) == "yay $(cat /etc/pkgdeck-aur-latest)" ]]
    ok aur list | row yay | jq -e '\''.update == "current"'\'' >/dev/null

    # Removing builds nothing, so no helper runs: Pacman removes the package
    # as root, the same sudo -n way. Not retried: a second try finds nothing.
    rm /usr/local/bin/paru
    out=$(run aur --yes remove pkgdeck-fixture-foreign || true)
    jq -e ".exit_code == 0" <<<"$out" >/dev/null || { echo "AUR remove failed: $out" >&2; exit 1; }
    if pacman -Q pkgdeck-fixture-foreign >/dev/null 2>&1; then echo "pkgdeck-fixture-foreign is still installed" >&2; exit 1; fi
    aur=$(ok aur list)
    if row pkgdeck-fixture-foreign <<<"$aur" >/dev/null; then echo "Removed package still listed" >&2; exit 1; fi
    row yay <<<"$aur" >/dev/null
    [[ $(pacman -Q yay) == "yay $(cat /etc/pkgdeck-aur-latest)" ]]
    echo "PASS real aur lifecycle"
'
# Arch's official image is x86_64 only. Emulated elsewhere, Pacman's download
# sandbox can't apply seccomp, so it is turned off there.
sandbox=
[[ $(uname -m) == x86_64 ]] || sandbox=--disable-sandbox
"$engine" run --rm --platform linux/amd64 -v "$binary:/opt/pkd:ro" docker.io/library/archlinux:base bash -c '
    # Mirrors time out now and then; setup is not what this tests.
    for attempt in 1 2 3; do
        if pacman -Sy --noconfirm $2 sudo fakeroot jq; then break; fi
        [[ $attempt == 3 ]] && exit 1
        sleep 15
    done
    exec bash -c "$1"
' bash "$script" "$sandbox"
