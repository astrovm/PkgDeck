#!/usr/bin/env bash
# Exercise one Wave 4/5 development manager through pkd against real tools.
# Ephemeral runners need no cleanup; lifecycle state is confirmed through
# the underlying manager, never only PkgDeck output. Tool installations use
# user-writable prefixes so no step ever needs elevation.
# Usage: scripts/tests/dev-manager.sh cargo|rustup|go|dotnet|npm|pnpm|bun|pip|pipx|uv|mise|pixi|conda|nix|composer|gem|rustup|nix <pkd>
# -E lets failures inside the helper functions below reach the ERR trap.
set -Eeuo pipefail
trap 'echo "dev-manager FAILED at line $LINENO: $BASH_COMMAND" >&2' ERR
backend=${1:?Usage: scripts/tests/dev-manager.sh cargo|rustup|go|dotnet|npm|pnpm|bun|pip|pipx|uv|mise|pixi|conda|nix|composer|gem|rustup|nix <pkd>}
pkd=${2:?Usage: scripts/tests/dev-manager.sh cargo|rustup|go|dotnet|npm|pnpm|bun|pip|pipx|uv|mise|pixi|conda|nix|composer|gem|rustup|nix <pkd>}
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

# rustup lives in the runner's ~/.cargo/bin; elsewhere a minimal install
# there is enough. The wrapper keeps assertions on that exact rustup.
setup_rustup() {
    local bin=${CARGO_HOME:-$HOME/.cargo}/bin
    if [[ ! -x $bin/rustup ]] && ! command -v rustup >/dev/null; then
        curl --proto '=https' --tlsv1.2 -fsSL https://sh.rustup.rs |
            sh -s -- -y --no-modify-path --profile minimal --default-toolchain stable
    fi
    rustup_bin=$(command -v rustup || echo "$bin/rustup")
    rustup() { "$rustup_bin" "$@"; }
    rustup --version
    rustup toolchain list
}

# Nix stays off PATH, the way a desktop app starts, so pkd has to find it
# in the profile the installer made. Linux gets the official single-user
# install; macOS needs the multi-user daemon. The Determinate installer
# dropped Intel Macs, so macOS uses the official one too.
setup_nix() {
    if [[ $(uname -s) == Darwin ]]; then
        nix_bin=/nix/var/nix/profiles/default/bin/nix
        if [[ ! -x $nix_bin ]]; then
            curl --proto '=https' --tlsv1.2 -fsSL -o "$HOME/nix-install" https://nixos.org/nix/install
            sh "$HOME/nix-install" --daemon --yes --no-modify-profile
        fi
    else
        nix_bin=$HOME/.nix-profile/bin/nix
        if [[ ! -x $nix_bin ]]; then
            curl --proto '=https' --tlsv1.2 -fsSL -o "$HOME/nix-install" https://nixos.org/nix/install
            sh "$HOME/nix-install" --no-daemon --no-modify-profile
        fi
    fi
    nix() { "$nix_bin" --extra-experimental-features 'nix-command flakes' "$@"; }
    nix --version
    # Nixpkgs 26.11 dropped Intel Macs; their users pin nixpkgs to the
    # 26.05 branch, which pkd then uses like any registry entry.
    if [[ $(uname -s) == Darwin && $(uname -m) == x86_64 ]]; then
        nix registry add nixpkgs github:NixOS/nixpkgs/nixpkgs-26.05-darwin
    fi
}

# `go install` writes to GOBIN, else the first GOPATH entry's bin; the
# runners ship Go, elsewhere the distribution package is enough.
setup_go() {
    if ! command -v go >/dev/null; then
        if [[ $(uname -s) == Darwin ]]; then brew install go
        else sudo apt-get update && sudo apt-get install -y golang-go; fi
    fi
    go version
    gobin=$(go env GOBIN)
    [[ -n $gobin ]] || gobin=$(go env GOPATH | cut -d: -f1)/bin
    echo "go bin directory: $gobin"
}

# The runners ship the .NET SDK; elsewhere the official script installs the
# LTS SDK into ~/.dotnet, off PATH, where pkd looks for it.
setup_dotnet() {
    export DOTNET_CLI_TELEMETRY_OPTOUT=1 DOTNET_NOLOGO=1
    if ! command -v dotnet >/dev/null && [[ ! -x $HOME/.dotnet/dotnet ]]; then
        curl -fsSL https://dot.net/v1/dotnet-install.sh -o "$HOME/dotnet-install.sh"
        bash "$HOME/dotnet-install.sh" --channel LTS --install-dir "$HOME/.dotnet" --no-path
    fi
    dotnet_bin=$(command -v dotnet || echo "$HOME/.dotnet/dotnet")
    dotnet() { "$dotnet_bin" "$@"; }
    dotnet --version
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
    # cowsay's dependencies sit next to it in node_modules but aren't tools.
    run list | absent '"name":"yargs"'
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
    success remove jq
    micromamba list --name tools --json | jq -e '[(.packages // .)[] | select(.name == "jq")] | length == 0'
    # The environment itself stays; only the package left it.
    micromamba env list --json | jq -e '.envs | any(endswith("/envs/tools"))'
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
rustup)
    # The checkout's rust-toolchain.toml would make rustup install its pin.
    cd "$HOME"
    setup_rustup
    success sources
    default=$(rustup default | awk '{print $1}')
    have "$default"
    # `rustup check` must be understood: the default reports a real version.
    run list | jq -e --arg name "$default" '.data.packages[] | select(.id.name == $name) | .installed_version | test("^[0-9]")'
    # A pinned release installs with the minimal profile and never moves.
    rustup toolchain list | absent '^1\.80\.0-'
    success install 1.80.0
    pinned=$(rustup toolchain list | awk '/^1\.80\.0-/ {print $1}')
    [[ -n $pinned ]]
    rustup run "$pinned" rustc --version | grep -q '^rustc 1\.80\.0 '
    rustup component list --toolchain "$pinned" --installed | absent '^rust-docs'
    have "$pinned"
    run list | jq -e --arg name "$pinned" '.data.packages[] | select(.id.name == $name) | .installed_version == "1.80.0" and .update == "current"'
    success info "$pinned"
    success upgrade "$pinned"
    rustup run "$pinned" rustc --version | grep -q '^rustc 1\.80\.0 '
    # The default toolchain is refused before rustup is asked to remove it.
    output=$(run remove "$default") || true
    grep -q 'is the default toolchain' <<<"$output"
    rustup toolchain list | grep -q "^$default"
    success remove "$pinned"
    rustup toolchain list | absent '^1\.80\.0-'
    ;;
nix)
    setup_nix
    success sources
    nix profile list --json | absent '"hello"'
    success install hello
    have hello
    nix profile list --json | jq -e '.elements.hello | .active and (.originalUrl == "flake:nixpkgs") and (.storePaths[0] | test("-hello-[0-9]"))'
    "$HOME/.nix-profile/bin/hello" | grep -qx 'Hello, world!'
    run info hello | jq -e '.data.package.installed_version | test("^[0-9]")'
    success upgrade hello
    nix profile list --json | jq -e '.elements.hello.active'
    # The single-user installer adds Nix itself as a store path, which has
    # no flake to upgrade from.
    if nix profile list --json | jq -e '.elements.nix | . != null and .originalUrl == null' >/dev/null; then
        output=$(run upgrade nix) || true
        grep -q 'installed from a store path' <<<"$output"
    fi
    success remove hello
    nix profile list --json | absent '"hello"'
    [[ ! -e $HOME/.nix-profile/bin/hello ]]
    ;;
go)
    setup_go
    success sources
    # An old release, so the proxy has a newer one to move to.
    go install golang.org/x/tools/cmd/stringer@v0.30.0
    have stringer
    run list | jq -e '.data.packages[] | select(.id.name == "stringer") | .installed_version == "v0.30.0" and .update == "available" and .id.reference == "golang.org/x/tools/cmd/stringer"'
    success info stringer
    success upgrade stringer
    go version -m "$gobin/stringer" | awk '$1 == "mod" {print $3}' | grep -qv '^v0\.30\.0$'
    # A file that isn't a Go program is neither listed nor removed.
    printf '#!/bin/sh\n' >"$gobin/not-go"
    chmod +x "$gobin/not-go"
    run list | absent '"not-go"'
    run remove not-go >/dev/null || true
    [[ -e $gobin/not-go ]]
    success remove stringer
    [[ ! -e $gobin/stringer ]]
    run list | absent '"stringer"'
    # A package path is an exact install offer.
    run search golang.org/x/tools/cmd/stringer | jq -e '.data.packages[] | select(.id.name == "stringer") | .candidate_version | startswith("v")'
    success install golang.org/x/tools/cmd/stringer
    have stringer
    go version -m "$gobin/stringer" | grep -q $'^\tpath\tgolang.org/x/tools/cmd/stringer$'
    success remove stringer
    [[ ! -e $gobin/stringer ]]
    [[ -e $gobin/not-go ]]
    ;;
dotnet)
    setup_dotnet
    success sources
    dotnet tool list --global | absent '^dotnetsay '
    # An old release, so the feed has a newer one to move to.
    dotnet tool install --global dotnetsay --version 2.1.7
    have dotnetsay
    run list | jq -e '.data.packages[] | select(.id.name == "dotnetsay") | .installed_version == "2.1.7" and .update == "available"'
    success info dotnetsay
    success upgrade dotnetsay
    dotnet tool list --global | awk '$1 == "dotnetsay" {print $2}' | grep -qv '^2\.1\.7$'
    success remove dotnetsay
    dotnet tool list --global | absent '^dotnetsay '
    run search dotnetsay | jq -e '.data.packages[] | select(.id.name == "dotnetsay") | .candidate_version | test("^[0-9]")'
    success install dotnetsay
    have dotnetsay
    dotnet tool list --global | grep -q '^dotnetsay '
    success remove dotnetsay
    dotnet tool list --global | absent '^dotnetsay '
    ;;
*) exit 2 ;;
esac
echo "PASS dev-manager $backend lifecycle"
