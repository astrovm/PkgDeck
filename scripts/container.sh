#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."
command -v podman >/dev/null || { echo 'Podman is required for --engine podman.' >&2; exit 1; }
[[ $(podman info --format '{{.Host.Security.Rootless}}') == true ]] || { echo 'Run Podman as an unprivileged user.' >&2; exit 1; }
kind=${1:?Expected development or lifecycle}; shift
case "$kind" in
    development) recipe=containers/development.Containerfile; inputs=("$recipe" rust-toolchain.toml scripts/setup-dev.sh scripts/dev-env.sh) ;;
    lifecycle) recipe=containers/lifecycle.Containerfile; inputs=("$recipe" scripts/vm/prepare.sh) ;;
    *) echo "Unknown container kind: $kind" >&2; exit 2 ;;
esac
key=$(cat "${inputs[@]}" | sha256sum | cut -c1-16)
image="localhost/pkgdeck-$kind:$key-$(uname -m)"
if ! podman image exists "$image"; then
    podman build --layers -f "$recipe" -t "$image" .
fi
if [[ "${1:-}" == --build-only ]]; then printf '%s\n' "$image"; exit 0; fi
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
    podman run --rm --cidfile "$run_directory/cid" --label io.pkgdeck.verification=true "$@" <&0 &
    if wait "$!"; then status=0; else status=$?; fi
    exit "$status"
}
if [[ "$kind" == development ]]; then
    workspace_key=$(printf '%s' "$PWD" | sha256sum | cut -c1-12)
    cache="${PKGDECK_CONTAINER_CACHE:-${XDG_CACHE_HOME:-$HOME/.cache}/pkgdeck/containers/$workspace_key/$key-$(uname -m)}"
    mkdir -p "$cache/cargo" "$cache/target"
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
    # No host HOME, credentials, daemon sockets, or package database is mounted.
    run_container "${interactive[@]}" --userns=keep-id -e LANG=C.UTF-8 \
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
    run_container "${gui_mount[@]}" -e "PKGDECK_FRONTEND=${PKGDECK_FRONTEND:-cli}" -v "$PWD:/mnt/pkgdeck:ro" -v "$binaries:/mnt/pkgdeck-bin:ro" "$image" \
        bash /mnt/pkgdeck/scripts/vm/guest.sh --container "$@"
fi
