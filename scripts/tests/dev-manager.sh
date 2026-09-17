#!/usr/bin/env bash
# Exercise one Wave 4 development manager through pkd against real tools.
# Ephemeral runners need no cleanup; lifecycle state is confirmed through
# the underlying manager, never only PkgDeck output. Tool installations use
# user-writable prefixes so no step ever needs elevation.
# Usage: scripts/tests/dev-manager.sh cargo|npm|pnpm|bun <pkd>
set -euo pipefail
trap 'echo "dev-manager FAILED at line $LINENO: $BASH_COMMAND" >&2' ERR
backend=${1:?Usage: scripts/tests/dev-manager.sh cargo|npm|pnpm|bun <pkd>}
pkd=${2:?Usage: scripts/tests/dev-manager.sh cargo|npm|pnpm|bun <pkd>}
echo "dev-manager: backend=$backend pkd=$pkd user=$(whoami) home=$HOME"
run() { "$pkd" --json --yes --auth sudo --from "$backend" "$@"; }
success() { run "$@" | grep -q '"exit_code":0'; }
have() { run list | grep -q "\"name\":\"$1\""; }

setup_node() {
    command -v npm >/dev/null || { sudo apt-get update && sudo apt-get install -y nodejs npm; }
    # Keep global installs inside the invoking user's home.
    npm config set prefix "$HOME/.npm-global"
    export PATH="$HOME/.npm-global/bin:$PATH"
}

case $backend in
cargo)
    success sources
    cargo install cowsay
    have cowsay
    success info cowsay
    success upgrade cowsay
    cargo install --list | grep -q '^cowsay v'
    success remove cowsay
    ! cargo install --list | grep -q '^cowsay v'
    ;;
npm)
    setup_node
    success sources
    npm install --global cowsay@1.5.0
    have cowsay
    success info cowsay
    success upgrade
    npm ls --global --depth=0 | grep -q 'cowsay@1.6.0'
    success remove cowsay
    ! npm ls --global --depth=0 2>/dev/null | grep -q 'cowsay@'
    success install cowsay
    have cowsay
    success remove cowsay
    ;;
pnpm)
    setup_node
    command -v pnpm >/dev/null || npm install --global pnpm
    # pnpm refuses global operations unless its bin directory is on PATH.
    export PNPM_HOME="$HOME/.local/share/pnpm"
    mkdir -p "$PNPM_HOME/bin"
    export PATH="$PNPM_HOME/bin:$PATH"
    command -v pnpm
    pnpm --version
    success sources
    pnpm add --global cowsay@1.5.0
    have cowsay
    success info cowsay
    success upgrade
    pnpm ls --global --depth=0 | grep -q 'cowsay@1.6.0'
    success remove cowsay
    ! pnpm ls --global --depth=0 | grep -q 'cowsay@'
    success install cowsay
    have cowsay
    success remove cowsay
    ;;
bun)
    setup_node
    if ! command -v bun >/dev/null; then
        curl -fsSL https://bun.sh/install -o /tmp/bun-install.sh \
            && bash /tmp/bun-install.sh || npm install --global bun
        export PATH="$HOME/.bun/bin:$PATH"
    fi
    command -v bun
    bun --version
    success sources
    bun add --global cowsay@1.5.0
    have cowsay
    success info cowsay
    success upgrade cowsay
    grep -q '"version": "1.6.0"' "$HOME/.bun/install/global/node_modules/cowsay/package.json"
    success remove cowsay
    ! test -e "$HOME/.bun/install/global/node_modules/cowsay"
    success install cowsay
    have cowsay
    success remove cowsay
    ;;
*) exit 2 ;;
esac
