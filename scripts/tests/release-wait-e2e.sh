#!/usr/bin/env bash
# Real managers, real registries: checks Wait for new releases end to end in a
# throwaway container with nothing from this machine but the `pkd` binary.
# Build it first, then: scripts/tests/release-wait-e2e.sh path/to/pkd
#
# Versions move, so nothing here names the newest version. Each case installs
# an old release, and compares what `pkd` offers with a long wait and none.
set -euo pipefail

if [[ ${1:-} != --inside ]]; then
    pkd=$(realpath "${1:?Usage: $0 path/to/pkd}")
    script=$(realpath "$0")
    image=${PKGDECK_E2E_IMAGE:-docker.io/library/ubuntu:26.04}
    exec podman run --rm --memory 12g --memory-swap 12g \
        -v "$pkd:/opt/pkd:ro,Z" -v "$script:/opt/e2e.sh:ro,Z" \
        "$image" bash /opt/e2e.sh --inside
fi

if [[ ${2:-} != --as-user ]]; then
    export DEBIAN_FRONTEND=noninteractive
    apt-get update -qq
    apt-get install -y -qq --no-install-recommends \
        ca-certificates curl unzip xz-utils jq python3-venv ruby build-essential >/dev/null
    # Ubuntu's npm is too old to limit release age, or to update itself.
    node=$(curl -fsSL https://nodejs.org/dist/index.json | jq -r '[.[] | select(.lts)][0].version')
    curl -fsSL "https://nodejs.org/dist/$node/node-$node-linux-x64.tar.xz" |
        tar -xJ --strip-components=1 -C /usr/local
    npm install -g npm@latest pnpm@latest
    npm --version
    useradd -m tester
    exec runuser -u tester -- bash /opt/e2e.sh --inside --as-user
fi

export PATH="$HOME/.local/bin:$HOME/.bun/bin:$HOME/.npm-global/bin:$HOME/.cargo/bin:$PATH"
export PNPM_HOME="$HOME/.local/share/pnpm" PATH="$HOME/.local/share/pnpm/bin:$HOME/.local/share/pnpm:$PATH"
npm config set prefix "$HOME/.npm-global"
npm config set ignore-scripts true
curl -fsSL https://astral.sh/uv/install.sh | UV_NO_MODIFY_PATH=1 sh >/dev/null 2>&1
curl -fsSL https://mise.run | MISE_INSTALL_PATH="$HOME/.local/bin/mise" sh >/dev/null 2>&1
curl -fsSL https://bun.sh/install | bash >/dev/null 2>&1
# Ubuntu's pipx is too old to report updates.
uv venv -q --seed "$HOME/pipx-tool"
"$HOME/pipx-tool/bin/pip" install -q pipx
ln -s "$HOME/pipx-tool/bin/pipx" "$HOME/.local/bin/pipx"

failures=0
check() { # what expected actual
    if [[ $2 == "$3" ]]; then
        echo "  ok    $1: $3"
    else
        echo "  FAIL  $1: expected $2, got $3"
        failures=$((failures + 1))
    fi
}
differs() { # what a b
    if [[ -n $2 && -n $3 && $2 != "$3" ]]; then
        echo "  ok    $1: $2 and $3"
    else
        echo "  FAIL  $1: expected two different versions, got '$2' and '$3'"
        failures=$((failures + 1))
    fi
}
pkd() { /opt/pkd "$@"; }
field() { # source name jq-field: from a fresh refresh
    pkd --json --from "$1" refresh |
        jq -r --arg name "$2" --arg field "$3" \
            '[.data.updates[], .data.waiting[]] | map(select(.id.name == $name)) | first | .[$field] // empty'
}
installed() { # source name
    pkd --json --from "$1" list |
        jq -r --arg name "$2" '.data.packages[] | select(.id.name == $name) | .installed_version'
}
ready_count() { pkd --json --from "$1" refresh | jq '.data.updates | length'; }
waiting_count() { pkd --json --from "$1" refresh | jq '.data.waiting | length'; }

# A manager that limits release age itself: with the wait on it offers and
# installs an old enough version, and nothing waits.
limited() { # source name
    local source=$1 name=$2 before newest limited_version
    before=$(installed "$source" "$name")
    pkd release-wait off >/dev/null
    newest=$(field "$source" "$name" candidate_version)
    pkd release-wait 30d >/dev/null
    limited_version=$(field "$source" "$name" candidate_version)
    differs "$source offers an older version with the wait on" "$limited_version" "$newest"
    check "$source has nothing waiting" 0 "$(waiting_count "$source")"
    pkd --from "$source" --yes upgrade >/dev/null 2>&1 || true
    check "$source installs the version it offered" "$limited_version" "$(installed "$source" "$name")"
    pkd release-wait off >/dev/null
    check "$source offers the newest again with the wait off" "$newest" "$(field "$source" "$name" candidate_version)"
    pkd --from "$source" --yes upgrade >/dev/null 2>&1 || true
    check "$source installs the newest with the wait off" "$newest" "$(installed "$source" "$name")"
    [[ $before != "$newest" ]] || echo "  note  $source: started on the newest version"
}

echo "== pkd release-wait"
check "off by default" false "$(pkd --json release-wait | jq .data.release_wait.enabled)"
pkd release-wait 12h >/dev/null
check "saves the time" 12 "$(pkd --json release-wait | jq .data.release_wait.hours)"
pkd release-wait --skip npm >/dev/null
check "saves a skipped source" '["npm"]' "$(pkd --json release-wait | jq -c .data.release_wait.skipped)"
pkd release-wait --no-skip npm off >/dev/null
check "turns off, keeps the time and unskips" 'false 12 []' "$(pkd --json release-wait | jq -r '.data.release_wait | "\(.enabled) \(.hours) \(.skipped)"')"
pkd release-wait apt >/dev/null 2>&1 && code=0 || code=$?
check "refuses nonsense" 2 "$code"

echo "== npm"
npm install -g --silent @anthropic-ai/claude-code@2.1.280 >/dev/null
limited npm @anthropic-ai/claude-code

echo "== uv"
uv tool install -q ruff==0.16.0
limited uv ruff

echo "== pnpm"
pnpm config set --global ignore-scripts true >/dev/null 2>&1 || true
pnpm add --global --silent --config.minimumReleaseAge=0 @anthropic-ai/claude-code@2.1.280 >/dev/null
limited pnpm @anthropic-ai/claude-code

echo "== mise"
mkdir -p "$HOME/.config/mise"
printf '[tools]\nruff = "0"\n' >"$HOME/.config/mise/config.toml"
mise trust -q -a >/dev/null 2>&1 || true
mise install -q ruff@0.16.0
rm -rf "$HOME/.local/share/mise/installs/ruff/latest"
limited mise ruff

echo "== pip"
uv venv -q --seed "$HOME/venv"
"$HOME/venv/bin/pip" install -q -U pip
"$HOME/venv/bin/pip" install -q ruff==0.16.0
(
    export VIRTUAL_ENV="$HOME/venv"
    limited pip ruff
)

# Sources PkgDeck holds back itself. pipx and Bun also get the limit when they
# install, so naming one stays within it.
echo "== pipx"
pipx install -q ruff==0.16.0 >/dev/null 2>&1
pkd release-wait 30d >/dev/null
check "pipx: ready before anything" 0 "$(ready_count pipx)"
check "pipx: waits" 1 "$(waiting_count pipx)"
pkd --from pipx --yes upgrade >/dev/null 2>&1 || true
check "pipx: upgrade leaves it" 0.16.0 "$(installed pipx ruff)"
pkd --from pipx --yes upgrade ruff >/dev/null 2>&1 || true
differs "pipx: by name stays within the limit" "$(installed pipx ruff)" "$(field pipx ruff candidate_version)"
pkd release-wait off >/dev/null
pipx uninstall -q ruff >/dev/null 2>&1 || true

echo "== Bun"
bun add --global --silent @anthropic-ai/claude-code@2.1.280 >/dev/null 2>&1
pkd release-wait 30d >/dev/null
check "Bun: waits" 1 "$(waiting_count bun)"
pkd --from bun --yes upgrade >/dev/null 2>&1 || true
check "Bun: upgrade leaves it" 2.1.280 "$(installed bun @anthropic-ai/claude-code)"
pkd --from bun --yes upgrade @anthropic-ai/claude-code >/dev/null 2>&1 || true
differs "Bun: by name stays within the limit" "$(installed bun @anthropic-ai/claude-code)" "$(field bun @anthropic-ai/claude-code candidate_version)"
pkd release-wait off >/dev/null

# RubyGems has no limit at all: only PkgDeck's wait stands in the way.
echo "== RubyGems"
gem install --user-install --no-document -v 3.0.0 rainbow >/dev/null 2>&1
gem install --user-install --no-document -v 13.0.0 rake >/dev/null 2>&1
export PATH="$(ruby -e 'print Gem.user_dir')/bin:$PATH"
newest=$(field gem rainbow candidate_version)
pkd release-wait 2d >/dev/null
check "gem: everything waits" 2 "$(waiting_count gem)"
pkd --from gem --yes upgrade >/dev/null 2>&1 || true
check "gem: upgrade leaves it" 3.0.0 "$(installed gem rainbow)"
# Time passes for one of them only.
seen="$HOME/.local/state/pkgdeck/release-wait-seen.json"
jq '(.[] | select(.id.name == "rake") | .at) = 1' "$seen" >"$seen.new"
mv "$seen.new" "$seen"
check "gem: the older one is ready" 1 "$(ready_count gem)"
check "gem: the other still waits" 1 "$(waiting_count gem)"
pkd --from gem --yes upgrade >/dev/null 2>&1 || true
check "gem: the waiting one stays" 3.0.0 "$(installed gem rainbow)"
if [[ $(installed gem rake) == 13.0.0 ]]; then
    echo "  FAIL  gem: the ready one should have updated"
    failures=$((failures + 1))
else
    echo "  ok    gem: the ready one updated to $(installed gem rake)"
fi
pkd --from gem --yes upgrade rainbow >/dev/null 2>&1 || true
check "gem: by name installs it right away" "$newest" "$(installed gem rainbow)"
pkd release-wait off >/dev/null

echo
if ((failures)); then
    echo "$failures check(s) failed"
    exit 1
fi
echo "All checks passed"
