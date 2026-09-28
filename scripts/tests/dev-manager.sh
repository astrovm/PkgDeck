#!/usr/bin/env bash
# Exercise one Wave 4/5 development manager through pkd against real tools.
# Ephemeral runners need no cleanup; lifecycle state is confirmed through
# the underlying manager, never only PkgDeck output. Tool installations use
# user-writable prefixes so no step ever needs elevation.
# Usage: scripts/tests/dev-manager.sh cargo|npm|pnpm|bun|pip|pipx|uv|mise|pixi|conda|composer|gem <pkd>
# -E lets failures inside the helper functions below reach the ERR trap.
set -Eeuo pipefail
trap 'echo "dev-manager FAILED at line $LINENO: $BASH_COMMAND" >&2' ERR
backend=${1:?Usage: scripts/tests/dev-manager.sh cargo|npm|pnpm|bun|pip|pipx|uv|mise|pixi|conda|composer|gem <pkd>}
pkd=${2:?Usage: scripts/tests/dev-manager.sh cargo|npm|pnpm|bun|pip|pipx|uv|mise|pixi|conda|composer|gem <pkd>}
echo "dev-manager: backend=$backend pkd=$pkd user=$(whoami) home=$HOME"
run() { "$pkd" --json --yes --auth sudo --from "$backend" "$@"; }
# Keep pkd's report so a failed step shows what pkd said and how long it took.
success() {
    local output start=$SECONDS
    output=$(run "$@") || true
    if ! grep -q '"exit_code":0' <<<"$output"; then
        echo "pkd $* failed after $((SECONDS - start))s: $output" >&2
        return 1
    fi
}
have() {
    local output start=$SECONDS
    output=$(run list) || true
    if ! grep -q "\"name\":\"$1\"" <<<"$output"; then
        echo "pkd list lacks $1 after $((SECONDS - start))s: $output" >&2
        return 1
    fi
}
absent() {
    if grep "$1" >/dev/null; then
        echo "Package still present after removal: $1" >&2
        return 1
    fi
}

setup_node() {
    if ! command -v npm >/dev/null; then
        if [[ $(uname -s) == Darwin ]]; then brew install node
        else sudo apt-get update && sudo apt-get install -y nodejs npm; fi
    fi
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
    if ! command -v pipx >/dev/null; then
        if [[ $(uname -s) == Darwin ]]; then brew install pipx
        else sudo apt-get update && sudo apt-get install -y pipx; fi
    fi
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
    if ! command -v composer >/dev/null; then
        if [[ $(uname -s) == Darwin ]]; then brew install composer
        else sudo apt-get update && sudo apt-get install -y php-cli php-zip unzip composer; fi
    fi
    export COMPOSER_HOME="$HOME/.config/composer"
    mkdir -p "$COMPOSER_HOME"
    composer --version
}

setup_mise() {
    command -v mise >/dev/null || curl -fsSL https://mise.run | sh
    export PATH="$HOME/.local/bin:$PATH"
    mise --version
}

# pixi's installer and micromamba's archive stay off PATH, the way a
# desktop app finds them, so pkd has to look in their usual places.
setup_pixi() {
    export PIXI_HOME="$HOME/.pixi"
    [[ -x $PIXI_HOME/bin/pixi ]] || curl -fsSL https://pixi.sh/install.sh | PIXI_NO_PATH_UPDATE=1 bash
    pixi() { "$PIXI_HOME/bin/pixi" "$@"; }
    pixi --version
}

setup_micromamba() {
    local platform
    case "$(uname -s)-$(uname -m)" in
    Darwin-arm64) platform=osx-arm64 ;;
    Darwin-*) platform=osx-64 ;;
    Linux-aarch64) platform=linux-aarch64 ;;
    *) platform=linux-64 ;;
    esac
    mkdir -p "$HOME/.local/bin"
    curl -fsSL "https://micro.mamba.pm/api/micromamba/$platform/latest" | tar -xj -C "$HOME/.local" bin/micromamba
    export MAMBA_ROOT_PREFIX="$HOME/micromamba"
    micromamba() { "$HOME/.local/bin/micromamba" "$@"; }
    micromamba --version
}

setup_gem() {
    if [[ $(uname -s) == Darwin ]]; then
        brew install ruby
        local ruby_prefix
        ruby_prefix=$(brew --prefix ruby)
        export PATH="$ruby_prefix/bin:$PATH"
    else
        command -v gem >/dev/null || { sudo apt-get update && sudo apt-get install -y ruby; }
    fi
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
    cargo install --list | absent '^cowsay v'
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
    npm ls --global --depth=0 | absent 'cowsay@'
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
    pnpm ls --global --depth=0 | absent 'cowsay@'
    success install cowsay
    have cowsay
    success remove cowsay
    ;;
bun)
    setup_node
    if ! command -v bun >/dev/null; then
        if ! { curl -fsSL https://bun.sh/install -o /tmp/bun-install.sh && bash /tmp/bun-install.sh; }; then
            npm install --global bun
        fi
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
    [[ ! -e $HOME/.bun/install/global/node_modules/cowsay ]]
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
    if "$VIRTUAL_ENV/bin/pip" show cowsay >/dev/null 2>&1; then exit 1; fi
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
    pipx list --short | absent 'cowsay'
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
    uv tool list | absent 'cowsay'
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
    composer global show --format=json | absent 'psr/log'
    success install psr/log
    have psr/log
    success remove psr/log
    ;;
mise)
    setup_mise
    success sources
    # A global request of "4" with only 4.40.5 installed leaves a newer 4.x
    # within the configured range, which is all an upgrade may move to.
    mise install yq@4.40.5
    mkdir -p "$HOME/.config/mise"
    printf '[tools]\nyq = "4"\n' >"$HOME/.config/mise/config.toml"
    have yq
    success info yq
    success upgrade
    mise ls --global --json yq | jq -e '.[0].version != "4.40.5"'
    # Upgrades never rewrite the configured request.
    grep -qx 'yq = "4"' "$HOME/.config/mise/config.toml"
    success remove yq
    mise ls --global --json | absent '"yq"'
    success search jq
    success install jq
    have jq
    grep -qx 'jq = "latest"' "$HOME/.config/mise/config.toml"
    success remove jq
    mise ls --json jq | jq -e 'length == 0'
    ;;
pixi)
    setup_pixi
    success sources
    # A range with a newer 14.x lets the update move within it, never to 15.
    pixi global install 'ripgrep==14.1.0'
    sed -i.bak 's/"==14.1.0"/">=14,<15"/' "$PIXI_HOME/manifests/pixi-global.toml"
    have ripgrep
    success info ripgrep
    success upgrade ripgrep
    pixi global list --json | jq -e '.[] | select(.name == "ripgrep") | .dependencies[] | select(.name == "ripgrep") | .version | startswith("14.") and . != "14.1.0"'
    success remove ripgrep
    pixi global list --json | absent '"ripgrep"'
    success install fd-find
    have fd-find
    success remove fd-find
    pixi global list --json | absent '"fd-find"'
    ;;
conda)
    setup_micromamba
    micromamba create --yes --quiet --name tools --channel conda-forge 'jq=1.7.1'
    success sources
    have jq
    success info jq
    success upgrade jq
    micromamba list --name tools --json | jq -e '(.packages // .)[] | select(.name == "jq") | .version != "1.7.1"'
    ;;
gem)
    setup_gem
    success sources
    gem install --user-install --no-document rake -v 13.0.0
    have rake
    success info rake
    success upgrade rake
    # System/default gems can remain after removing the user-owned copy.
    # Assert the exact installation scope that PkgDeck manages.
    user_gemhome=$(gem env user_gemhome)
    find "$user_gemhome/specifications" -name 'rake-*.gemspec' | grep -v '/rake-13.0.0.gemspec$'
    success remove rake
    find "$user_gemhome/specifications" -name 'rake-*.gemspec' | absent '.'
    success install cowsay
    have cowsay
    success remove cowsay
    find "$user_gemhome/specifications" -name 'cowsay-*.gemspec' | absent '.'
    ;;
*) exit 2 ;;
esac
