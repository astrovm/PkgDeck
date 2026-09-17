#!/usr/bin/env bash
# Exercise one Wave 4/5 development manager through pkd against real tools.
# Ephemeral runners need no cleanup; lifecycle state is confirmed through
# the underlying manager, never only PkgDeck output. Tool installations use
# user-writable prefixes so no step ever needs elevation.
# Usage: scripts/tests/dev-manager.sh cargo|npm|pnpm|bun|pip|pipx|uv|composer|gem <pkd>
set -euo pipefail
trap 'echo "dev-manager FAILED at line $LINENO: $BASH_COMMAND" >&2' ERR
backend=${1:?Usage: scripts/tests/dev-manager.sh cargo|npm|pnpm|bun|pip|pipx|uv|composer|gem <pkd>}
pkd=${2:?Usage: scripts/tests/dev-manager.sh cargo|npm|pnpm|bun|pip|pipx|uv|composer|gem <pkd>}
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

setup_python() {
    # Provision virtual environments through uv's managed interpreter so the
    # test runner needs no system Python packages or elevation.
    command -v uv >/dev/null || curl -LsSf https://astral.sh/uv/install.sh | sh
    export PATH="$HOME/.local/bin:$PATH"
    uv venv --help >/dev/null
}

setup_pipx() {
    setup_python
    command -v pipx >/dev/null || { sudo apt-get update && sudo apt-get install -y pipx; }
    export PIPX_HOME="$HOME/.local/share/pipx"
    export PIPX_BIN_DIR="$HOME/.local/bin"
    export PATH="$PIPX_BIN_DIR:$PATH"
}

setup_uv() {
    command -v uv >/dev/null || curl -LsSf https://astral.sh/uv/install.sh | sh
    export PATH="$HOME/.local/bin:$PATH"
    command -v uv
    uv --version
}

setup_composer() {
    command -v composer >/dev/null || { sudo apt-get update && sudo apt-get install -y php-cli php-zip unzip composer; }
    export COMPOSER_HOME="$HOME/.config/composer"
    mkdir -p "$COMPOSER_HOME"
    composer --version
}

setup_gem() {
    command -v gem >/dev/null || { sudo apt-get update && sudo apt-get install -y ruby; }
    gem --version
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
pip)
    setup_python
    # pip is pinned to one explicitly selected virtual environment.
    uv venv --seed --quiet "$HOME/pkd-venv"
    export VIRTUAL_ENV="$HOME/pkd-venv"
    "$VIRTUAL_ENV/bin/pip" install --quiet 'cowsay==6.0'
    success sources
    have cowsay
    success info cowsay
    success upgrade
    "$VIRTUAL_ENV/bin/pip" show cowsay | grep 'Version: 6.1'
    success remove cowsay
    ! "$VIRTUAL_ENV/bin/pip" show cowsay >/dev/null 2>&1
    success install cowsay
    have cowsay
    success remove cowsay
    ;;
pipx)
    setup_pipx
    success sources
    pipx install --quiet 'cowsay==6.0'
    have cowsay
    success info cowsay
    success upgrade
    pipx list --short | grep 'cowsay 6.1'
    success remove cowsay
    ! pipx list --short 2>/dev/null | grep -q 'cowsay'
    success install cowsay
    have cowsay
    success remove cowsay
    ;;
uv)
    setup_uv
    export UV_TOOL_DIR="$HOME/.local/share/uv/tools"
    success sources
    uv tool install --quiet 'cowsay==6.0'
    have cowsay
    success info cowsay
    success upgrade
    uv tool list | grep 'cowsay v6.1'
    success remove cowsay
    ! uv tool list 2>/dev/null | grep -q 'cowsay'
    success install cowsay
    have cowsay
    success remove cowsay
    ;;
composer)
    setup_composer
    success sources
    composer global require --no-interaction --no-progress psr/log:1.0.0
    have psr/log
    success info psr/log
    success upgrade
    composer global show --format=json | grep '"version": "3'
    success remove psr/log
    ! composer global show --format=json 2>/dev/null | grep -q 'psr/log'
    success install psr/log
    have psr/log
    success remove psr/log
    ;;
gem)
    setup_gem
    success sources
    gem install --user-install --no-document rake -v 13.0.0
    have rake
    success info rake
    success upgrade rake
    gem list rake | grep 'rake (' | grep -v '(13.0.0)$'
    success remove rake
    ! gem list 2>/dev/null | grep -q '^rake '
    success install cowsay
    have cowsay
    success remove cowsay
    ;;
*) exit 2 ;;
esac
