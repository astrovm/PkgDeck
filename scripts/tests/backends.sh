#!/usr/bin/env bash
# Run one platform's backend lifecycle tests in a single CI job. A job per
# backend spent most of its time waiting for a runner and setting up, not
# testing. Each backend still gets its own log group and result, and every
# backend runs even after another fails, so one job reports all failures.
# Usage: scripts/tests/backends.sh native|dev <pkd> [musl pkd]
# Alpine (musl) and Void need the static musl pkd from the third argument.
set -uo pipefail
group=${1:?Usage: scripts/tests/backends.sh native|dev <pkd> [musl pkd]}
pkd=${2:?Usage: scripts/tests/backends.sh native|dev <pkd> [musl pkd]}
musl=${3:-$pkd}
here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
case $group in
native)
    [[ $(uname -s) == Linux ]] || { echo 'Native managers are tested on Linux only' >&2; exit 2; }
    backends=(dnf zypper apk xbps snap toolbox distrobox)
    # Arch's official container (Pacman and the AUR) is x86_64-only.
    [[ $(uname -m) == x86_64 ]] && backends=(dnf pacman aur zypper apk xbps snap toolbox distrobox)
    ;;
dev)
    backends=(cargo rustup go dotnet npm pnpm bun pip pipx uv mise pixi conda nix composer gem)
    # MacPorts rides along on macOS rather than taking another macOS runner.
    [[ $(uname -s) == Darwin ]] && backends+=(macports)
    ;;
*) echo "Expected native or dev, got $group" >&2; exit 2 ;;
esac

failed=()
summary=${GITHUB_STEP_SUMMARY:-/dev/null}
printf '| Backend | Result | Time |\n| --- | --- | --- |\n' >>"$summary"
for backend in "${backends[@]}"; do
    start=$SECONDS
    echo "::group::$backend"
    case $backend in
    dnf | pacman | zypper) "$here/native-manager.sh" "$backend" "$pkd" ;;
    apk | xbps) "$here/native-manager.sh" "$backend" "$musl" ;;
    aur) "$here/aur-manager.sh" "$pkd" ;;
    snap) "$here/snap-manager.sh" "$pkd" ;;
    toolbox | distrobox) "$here/dev-containers.sh" "$backend" "$pkd" ;;
    macports) "$here/macports-manager.sh" "$pkd" ;;
    *) "$here/dev-manager.sh" "$backend" "$pkd" ;;
    esac
    status=$?
    echo '::endgroup::'
    took=$((SECONDS - start))
    if ((status == 0)); then
        echo "PASS $backend (${took}s)"
        printf '| %s | ✅ pass | %ss |\n' "$backend" "$took" >>"$summary"
    else
        echo "::error title=$backend backend failed::$backend lifecycle test exited with $status after ${took}s"
        printf '| %s | ❌ fail | %ss |\n' "$backend" "$took" >>"$summary"
        failed+=("$backend")
    fi
done
if ((${#failed[@]})); then
    echo "Failed backends: ${failed[*]}" >&2
    exit 1
fi
echo "All ${#backends[@]} backends passed"
