#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."
command -v podman >/dev/null || { echo 'Podman is required for --engine podman.' >&2; exit 1; }
[[ $(podman info --format '{{.Host.Security.Rootless}}') == true ]] || { echo 'Run Podman as an unprivileged user.' >&2; exit 1; }
kind=${1:?Expected development or lifecycle}; shift
case "$kind" in
    development) recipe=containers/development.Containerfile; inputs=("$recipe" rust-toolchain.toml scripts/setup-dev.sh scripts/dev-env.sh) ;;
    lifecycle) recipe=containers/lifecycle.Containerfile; inputs=("$recipe" scripts/ci/apt-mirrors.sh scripts/vm/prepare.sh) ;;
    *) echo "Unknown container kind: $kind" >&2; exit 2 ;;
esac
key=$(cat "${inputs[@]}" | sha256sum | cut -c1-16)
image="localhost/pkgdeck-$kind:$key-$(uname -m)"
# CI keeps built images in PKGDECK_IMAGE_ARCHIVE, so a slow package mirror
# costs one build per recipe change instead of one per run.
archive=${PKGDECK_IMAGE_ARCHIVE:+$PKGDECK_IMAGE_ARCHIVE/pkgdeck-$kind-$key-$(uname -m).tar}
if ! podman image exists "$image"; then
    if [[ -n $archive && -s $archive ]]; then
        podman load -i "$archive"
    else
        podman build --layers -f "$recipe" -t "$image" .
        if [[ -n $archive ]]; then
            mkdir -p "$PKGDECK_IMAGE_ARCHIVE"
            podman save -o "$archive" "$image"
        fi
    fi
fi
if [[ "${1:-}" == --build-only ]]; then printf '%s\n' "$image"; exit 0; fi
# A local run leaves half the machine free: by default it gets half of the
# engine's CPUs and memory. CI runners are disposable, so they get all of it.
# PKGDECK_CONTAINER_CPUS and PKGDECK_CONTAINER_MEMORY (bytes, or 4g) change
# the limits; 0 lifts one.
limits=()
if [[ -z ${CI:-} ]]; then
    host=$(podman info --format '{{.Host.CPUs}} {{.Host.MemTotal}}')
    host_cpus=${host% *}
    host_memory=${host#* }
    default_cpus=$((host_cpus > 1 ? host_cpus / 2 : 1))
    default_memory=$((host_memory / 2))
fi
cpus=${PKGDECK_CONTAINER_CPUS:-${default_cpus:-0}}
memory=${PKGDECK_CONTAINER_MEMORY:-${default_memory:-0}}
[[ $cpus == 0 ]] || limits+=("--cpus=$cpus")
[[ $memory == 0 ]] || limits+=("--memory=$memory")
run_container() {
    local status
    run_directory=$(mktemp -d "${TMPDIR:-/tmp}/pkgdeck-container-XXXXXX")
    cleanup() {
        if [[ -s "$run_directory/cid" ]]; then
            podman rm --force --ignore --time 1 --cidfile "$run_directory/cid" >/dev/null 2>&1 || true
        fi
        rm -rf "$run_directory"
    }
    trap cleanup EXIT
    trap 'exit 130' INT
    trap 'exit 143' TERM
    podman run --rm --cidfile "$run_directory/cid" --label io.pkgdeck.verification=true \
        ${limits[@]+"${limits[@]}"} "$@" <&0 &
    if wait "$!"; then status=0; else status=$?; fi
    exit "$status"
}
if [[ "$kind" == development ]]; then
    workspace_key=$(printf '%s' "$PWD" | sha256sum | cut -c1-12)
    caches="${XDG_CACHE_HOME:-$HOME/.cache}/pkgdeck/containers"
    cache="${PKGDECK_CONTAINER_CACHE:-$caches/$workspace_key/$key-$(uname -m)}"
    mkdir -p "$cache/cargo"
    # Hold a shared lock for the entire run. Only an exclusive holder may
    # replace target, so another wrapper cannot delete it under a container.
    exec 9>"$cache/.target-lock"
    if flock -n -x 9; then
        if [[ -d $cache/target ]]; then
            # -k is POSIX: macOS du has no -b.
            target_kib=$(du -sk "$cache/target" | cut -f1)
            if ((target_kib > 20000000000 / 1024)); then
                echo "Deleting oversized container target: $cache/target ($target_kib KiB)" >&2
                rm -rf "$cache/target"
            fi
        fi
        mkdir -p "$cache/target"
        flock -s 9
    else
        flock -s 9
        mkdir -p "$cache/target"
    fi
    if [[ -z ${PKGDECK_CONTAINER_CACHE:-} ]]; then
        scripts/prune-container-caches.sh "$caches" "$workspace_key" "$key-$(uname -m)" "$PWD"
    fi
    if [[ "${1:-}" == --binary-dir ]]; then printf '%s/target/debug\n' "$cache"; exit 0; fi
    command=(scripts/verify.sh "$@")
    interactive=()
    if [[ "${1:-}" == --exec ]]; then
        shift
        (($#)) || { echo 'Expected a command after --exec' >&2; exit 2; }
        command=("$@")
    elif [[ "${1:-}" == --shell ]]; then
        command=(bash)
        interactive=(-it)
    fi
    if [[ "${1:-}" == --build-cli ]]; then command=(cargo build --locked -p pkd -p pkgdeck-tools); fi
    # Podman would otherwise create this mount point as the container's root,
    # a mapped user that then keeps native builds out of the checkout.
    mkdir -p target
    # A worktree's .git is a file that points into the main checkout's git
    # folder, maybe by a relative path. Mount that folder read-only at the
    # same path, and a .git file with its absolute path over the worktree's,
    # so git works in the container as in a plain clone (scripts/mutants.sh).
    git_mount=()
    if [[ -f .git ]]; then
        common=$(git rev-parse --path-format=absolute --git-common-dir)
        # One file per worktree: several can share a PKGDECK_CONTAINER_CACHE.
        git_file="$cache/git-file-$workspace_key"
        printf 'gitdir: %s\n' "$(git rev-parse --absolute-git-dir)" >"$git_file.$$"
        mv "$git_file.$$" "$git_file"
        git_mount=(-v "$common:$common:ro" -v "$git_file:/workspace/.git:ro")
    fi
    # No host HOME, credentials, daemon sockets, or package database is mounted.
    # The `[@]+` form keeps an empty array from being an unbound variable: the
    # default macOS bash is 3.2, where set -u rejects "${interactive[@]}".
    run_container ${interactive[@]+"${interactive[@]}"} ${git_mount[@]+"${git_mount[@]}"} \
        --userns=keep-id -e LANG=C.UTF-8 \
        -v "$PWD:/workspace:rw" -v "$cache/cargo:/cache/cargo:rw" \
        -v "$cache/target:/workspace/target:rw" \
        "$image" bash -ec 'mkdir -p "$HOME"; source scripts/dev-env.sh; exec "$@"' -- "${command[@]}"
else
    # Root in this user namespace is unprivileged on the host. Package tools and
    # sudo grants can change only the disposable container's filesystem.
    binaries="${PKGDECK_BINARY_DIR:-${CARGO_TARGET_DIR:-$PWD/target}/debug}"
    binaries=$(realpath "$binaries")
    gui_mount=()
    if [[ "${PKGDECK_FRONTEND:-cli}" == gui ]]; then
        appdir=$(realpath "${PKGDECK_APPDIR:-$PWD/build/AppDir}")
        [[ -x "$appdir/AppRun" ]] || { echo 'Stage the GUI with scripts/bundle.sh first.' >&2; exit 1; }
        gui_mount=(-v "$appdir:/mnt/pkgdeck-app:ro")
    fi
    tools_binary=$(realpath "${PKGDECK_TOOLS_BINARY:-${CARGO_TARGET_DIR:-target}/debug/pkgdeck-tools}")
    gui_mount+=(-v "$tools_binary:/mnt/pkgdeck-tools:ro")
    run_container ${gui_mount[@]+"${gui_mount[@]}"} -e "PKGDECK_FRONTEND=${PKGDECK_FRONTEND:-cli}" -v "$PWD:/mnt/pkgdeck:ro" -v "$binaries:/mnt/pkgdeck-bin:ro" "$image" \
        bash /mnt/pkgdeck/scripts/vm/guest.sh --container "$@"
fi
