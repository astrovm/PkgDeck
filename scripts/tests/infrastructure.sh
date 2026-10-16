#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/../.."
scripts/tests/release-packages.sh
scripts/tests/homebrew-style.sh
# KDE opens links and package files with the highest InitialPreference
# when nobody picked an app. Discover's Flatpak link handler has 5.
preference=$(sed -n 's/^InitialPreference=//p' assets/io.github.astrovm.PkgDeck.desktop)
(( ${preference:-0} > 5 )) || { echo "InitialPreference must beat Discover's 5"; exit 1; }
# Flatpak shows the version of the newest AppStream release, so each
# version bump adds one.
version=$(sed -n 's/^version = "\(.*\)"$/\1/p' Cargo.toml | head -1)
release=$(grep -o '<release version="[^"]*"' assets/io.github.astrovm.PkgDeck.metainfo.xml | head -1)
[[ $release == "<release version=\"$version\"" ]] || {
    echo "The newest AppStream release must be $version, found ${release:-none}"
    exit 1
}
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
expect_code() {
    local expected=$1
    shift
    local actual=0
    "$@" >"$work/output" 2>&1 || actual=$?
    [[ $actual == "$expected" ]] || {
        cat "$work/output"
        echo "Expected $expected, got $actual"
        exit 1
    }
}
for args in --help unknown 'fast --unknown'; do
    expected=2
    [[ $args != --help ]] || expected=0
    read -ra words <<<"$args"
    expect_code "$expected" scripts/verify.sh "${words[@]}"
    grep -q Usage: "$work/output"
done
mkdir "$work/bin"
printf '#!/bin/sh\necho synthetic-format-error\nexit 23\n' >"$work/bin/cargo"
chmod +x "$work/bin/cargo"
expect_code 23 env PATH="$work/bin:$PATH" PKGDECK_LOG_ROOT="$work/logs" scripts/verify.sh fast
grep -q 'FAIL format' "$work/output"
grep -q synthetic-format-error "$work/logs/latest/format.log"
[[ ! -e $work/logs/latest/test-terminal.log ]]
printf '#!/bin/sh\necho synthetic-slow-stage\nexec sleep 30\n' >"$work/bin/cargo"
expect_code 124 env PATH="$work/bin:$PATH" PKGDECK_LOG_ROOT="$work/slow" PKGDECK_STAGE_TIMEOUT=0.1s scripts/verify.sh fast
grep -q synthetic-slow-stage "$work/slow/latest/format.log"
# Exercise the real container wrapper with a synthetic engine: no image or
# desktop is needed to check forwarding, mounts, and failure propagation.
cat >"$work/bin/podman" <<'EOF'
#!/bin/bash
case "$1" in
    info) echo true ;;
    image) exit 0 ;;
    run) printf '%s\n' "$@" >"$PODMAN_ARGS"; exit "${PODMAN_STATUS:-0}" ;;
    *) exit 2 ;;
esac
EOF
chmod +x "$work/bin/podman"
expect_code 23 env PATH="$work/bin:$PATH" PODMAN_ARGS="$work/args" PODMAN_STATUS=23 PKGDECK_CONTAINER_CACHE="$work/cache" PKGDECK_LOG_ROOT="$work/container-logs" scripts/verify.sh full --only tests --engine podman
grep -Fxq -- '--only' "$work/args"
grep -Fxq tests "$work/args"
grep -Fxq "$work/cache/cargo:/cache/cargo:rw" "$work/args"
grep -Fxq "$work/cache/target:/workspace/target:rw" "$work/args"
grep -q 'CARGO_HOME=/cache/cargo' containers/development.Containerfile
expect_code 0 env PATH="$work/bin:$PATH" PODMAN_ARGS="$work/args" PKGDECK_CONTAINER_CACHE="$work/cache" scripts/container.sh development --exec cargo test --locked example
grep -Fxq example "$work/args"
expect_code 2 env PATH="$work/bin:$PATH" PKGDECK_CONTAINER_CACHE="$work/cache" scripts/container.sh development --exec
expect_code 1 scripts/tests/host-authorization.sh
grep -q 'disposable PkgDeck GitHub-hosted runner' "$work/output"
echo 'PASS verifier arguments, failure, timeout, and hosted-runner guard'
