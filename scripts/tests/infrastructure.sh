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
# cargo install inside this checkout names the pinned toolchain or runs from
# elsewhere: otherwise rust-toolchain.toml picks it and cargo warns.
bare=$(grep -rnE 'cargo install( |$)' scripts containers .github \
    | grep -vE ':[0-9]+:[[:space:]]*#|cargo "\+[$]toolchain" install|cd / && cargo install|install --list' || true)
[[ -z $bare ]] || { printf 'Name the pinned toolchain (cargo +TOOLCHAIN install) or install from outside the checkout:\n%s\n' "$bare"; exit 1; }
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
    info) [[ $* == *CPUs* ]] && echo '8 1000' || echo true ;;
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
# Without a chosen cache, a run deletes the caches nobody uses any more: an
# older recipe's after a day, another checkout's once it's gone or after two
# weeks. What is in use, or was used lately, stays.
caches="$work/xdg/pkgdeck/containers"
workspace_key=$(printf '%s' "$PWD" | sha256sum | cut -c1-12)
mkdir -p "$caches/$workspace_key/old-recipe/target" "$caches/$workspace_key/recent-recipe/target" \
    "$caches/gone/x/target" "$caches/idle/x/target" "$caches/elsewhere/x/target" "$caches/unmarked/x"
touch -d '3 days ago' "$caches/$workspace_key/old-recipe/.last-used"
touch "$caches/$workspace_key/recent-recipe/.last-used"
printf '%s\n' "$work/no-such-checkout" >"$caches/gone/workspace"
printf '%s\n' "$work" >"$caches/idle/workspace" "$caches/elsewhere/workspace"
touch -d '20 days ago' "$caches/idle/x/.last-used"
touch "$caches/elsewhere/x/.last-used"
touch -d '20 days ago' "$caches/unmarked/x" "$caches/unmarked"
expect_code 0 env -u PKGDECK_CONTAINER_CACHE PATH="$work/bin:$PATH" PODMAN_ARGS="$work/args" XDG_CACHE_HOME="$work/xdg" \
    scripts/container.sh development --exec true
# The one recipe folder the run kept besides the recent one.
for entry in "$caches/$workspace_key"/*; do
    case ${entry##*/} in recent-recipe | workspace) ;; *) current=${entry##*/} ;; esac
done
[[ -f $caches/$workspace_key/$current/.last-used && -d $caches/$workspace_key/$current/target ]]
[[ $(<"$caches/$workspace_key/workspace") == "$PWD" ]]
[[ ! -e $caches/$workspace_key/old-recipe && -d $caches/$workspace_key/recent-recipe ]]
[[ ! -e $caches/gone && ! -e $caches/idle && ! -e $caches/unmarked && -d $caches/elsewhere ]]
grep -q "Deleting an old container cache: $caches/$workspace_key/old-recipe" "$work/output"
# A local run gets half the engine's CPUs and memory unless told otherwise.
for setting in local CI=true PKGDECK_CONTAINER_CPUS=0 'PKGDECK_CONTAINER_CPUS=3 PKGDECK_CONTAINER_MEMORY=4g'; do
    read -ra overrides <<<"${setting/local/}"
    expect_code 0 env -u CI PATH="$work/bin:$PATH" PODMAN_ARGS="$work/args" PKGDECK_CONTAINER_CACHE="$work/cache" \
        ${overrides[@]+"${overrides[@]}"} scripts/container.sh development --exec true
    grep -E -- '^--(cpus|memory)=' "$work/args" | tr '\n' ' ' >"$work/limits" || true
    case $setting in
        local) expected='--cpus=4 --memory=500 ' ;;
        CI=true) expected='' ;;
        PKGDECK_CONTAINER_CPUS=0) expected='--memory=500 ' ;;
        *) expected='--cpus=3 --memory=4g ' ;;
    esac
    [[ $(<"$work/limits") == "$expected" ]] || { echo "$setting: got '$(<"$work/limits")', expected '$expected'"; exit 1; }
done
# Releases need the pull request that brought a commit to main, directly or
# as part of a stack, found here through a synthetic GitHub API.
cat >"$work/bin/gh" <<'EOF'
#!/bin/bash
pull() { printf '[{"number":%s,"merged_at":%s,"merge_commit_sha":"%s","base":{"ref":"%s"},"head":{"sha":"head-%s","repo":{"full_name":"o/r"}}}]' "$@" "$3"; }
below() { printf '[{"merged_at":%s,"base":{"ref":"%s"}}]' "$@"; }
case "$*" in
    "api repos/o/r/commits/direct/pulls") pull 1 '"t"' direct main ;;
    "api repos/o/r/commits/stacked/pulls") pull 2 '"t"' stacked middle ;;
    "api repos/o/r/commits/stray/pulls") pull 3 '"t"' stray abandoned ;;
    "api repos/o/r/commits/looped/pulls") pull 4 '"t"' looped loop ;;
    "api repos/o/r/commits/unmerged/pulls") pull 5 null unmerged main ;;
    *head=o:middle*) below '"t"' bottom ;;
    *head=o:bottom*) below '"t"' main ;;
    *head=o:abandoned*) below null main ;;
    *head=o:loop*) below '"t"' loop ;;
    *) exit 1 ;;
esac
EOF
chmod +x "$work/bin/gh"
for sha in direct stacked stray looped unmerged; do
    expected=1
    [[ $sha == direct || $sha == stacked ]] && expected=0
    expect_code "$expected" env PATH="$work/bin:$PATH" GITHUB_REPOSITORY=o/r scripts/merged-pr.sh "$sha"
done
# The stacked pull request is the one named, not the one below it.
expect_code 0 env PATH="$work/bin:$PATH" GITHUB_REPOSITORY=o/r scripts/merged-pr.sh stacked
grep -Fxq $'2\thead-stacked\to/r' "$work/output"
expect_code 1 scripts/tests/host-authorization.sh
grep -q 'disposable PkgDeck GitHub-hosted runner' "$work/output"
echo 'PASS verifier arguments, failure, timeout, and hosted-runner guard'
