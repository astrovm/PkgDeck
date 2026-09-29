#!/usr/bin/env bash
# Exercise Toolbx or Distrobox development containers through pkd against the
# real tools: list, details, upgrade, and removal.
# Results are confirmed through the tool itself, never only PkgDeck output.
# Each test container is made with its own tool and removed at exit, even on
# failure. A second container, made from the same Toolbx image by the other
# tool, must never show up under this source.
# Usage: scripts/tests/dev-containers.sh toolbox|distrobox <pkd>
# -E lets failures inside the helper functions below reach the ERR trap.
set -Eeuo pipefail
trap 'echo "dev-containers FAILED at line $LINENO: $BASH_COMMAND" >&2' ERR
usage="Usage: scripts/tests/dev-containers.sh toolbox|distrobox <pkd>"
backend=${1:?$usage}
pkd=${2:?$usage}
case $backend in
toolbox | distrobox) ;;
*)
    echo "$usage" >&2
    exit 2
    ;;
esac
echo "dev-containers: backend=$backend pkd=$pkd user=$(whoami) home=$HOME"

# Toolbx images carry the com.github.containers.toolbox label, so a Distrobox
# container made from one is the case that could appear twice.
toolbx_image=quay.io/toolbx/ubuntu-toolbox:24.04
distrobox_image=docker.io/library/alpine:3.22
box="pkd-test-$backend"
other="pkd-test-other"
exported=$HOME/.local/bin/pkd-test-busybox

cleanup() {
    podman rm --force --time 0 "$box" "$other" >/dev/null 2>&1 || true
    rm -f "$exported"
}
trap cleanup EXIT

run() { "$pkd" --json --yes --from "$backend" "$@"; }
# Keep pkd's report so a failed step shows what pkd said and how long it took.
success() {
    local output start=$SECONDS
    output=$(run "$@") || true
    if ! grep -q '"exit_code":0' <<<"$output"; then
        echo "pkd $* failed after $((SECONDS - start))s: $output" >&2
        return 1
    fi
    printf '%s\n' "$output"
}
listed() { grep -q "\"name\":\"$1\"" <<<"$2"; }

if ! command -v podman >/dev/null || ! command -v toolbox >/dev/null || ! command -v distrobox >/dev/null; then
    sudo apt-get update
    sudo apt-get install -y podman podman-toolbox distrobox
fi
# Toolbx keeps its state under XDG_RUNTIME_DIR; runner shells may lack it.
uid=$(id -u)
if [[ -z ${XDG_RUNTIME_DIR:-} && -d /run/user/$uid ]]; then
    export XDG_RUNTIME_DIR=/run/user/$uid
fi
podman --version
toolbox --version
distrobox version
cleanup

case $backend in
toolbox)
    toolbox create --assumeyes --image "$toolbx_image" --container "$box"
    distrobox create --yes --name "$other" --image "$toolbx_image"
    ;;
distrobox)
    distrobox create --yes --name "$box" --image "$distrobox_image"
    toolbox create --assumeyes --image "$toolbx_image" --container "$other"
    # Set the container up and export a command, which details must name.
    distrobox enter "$box" -- distrobox-export --bin /bin/busybox --export-path "$HOME/.local/bin"
    mv "$HOME/.local/bin/busybox" "$exported"
    ;;
esac

sources=$(success sources)
grep -q "\"backend\":\"$backend\"" <<<"$sources"
list=$(success list)
listed "$box" "$list" || {
    echo "pkd list lacks $box: $list" >&2
    exit 1
}
if listed "$other" "$list"; then
    echo "pkd list shows $other, which the other tool made: $list" >&2
    exit 1
fi

info=$(success info "$box")
case $backend in
toolbox) grep -q "Made from $toolbx_image" <<<"$info" ;;
distrobox)
    grep -q "Made from $distrobox_image" <<<"$info"
    grep -q "pkd-test-busybox in ~/.local/bin" <<<"$info"
    ;;
esac

success upgrade "$box" >/dev/null
podman container exists "$box"
case $backend in
toolbox)
    pending=$(toolbox run --container "$box" sh -c 'apt-get -s upgrade | grep -c "^Inst" || true')
    ;;
distrobox)
    pending=$(distrobox enter "$box" -- sh -c 'apk list --upgradable 2>/dev/null | wc -l')
    ;;
esac
if [[ $pending -ne 0 ]]; then
    echo "$box still has $pending packages to upgrade after pkd upgrade" >&2
    exit 1
fi

# Removing deletes the running container, and only that one.
success remove "$box" >/dev/null
if podman container exists "$box"; then
    echo "$box still exists after pkd remove" >&2
    exit 1
fi
podman container exists "$other"
list=$(success list)
if listed "$box" "$list"; then
    echo "pkd list still shows $box after pkd remove" >&2
    exit 1
fi
# A container that is gone can't be removed again.
removal=$(run remove "$box") || true
if grep -q '"exit_code":0' <<<"$removal"; then
    echo "pkd removed $box twice: $removal" >&2
    exit 1
fi
echo "dev-containers: $backend passed"
