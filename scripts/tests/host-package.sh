#!/usr/bin/env bash
# Verify host reads and disposable host writes through the actual packaged CLI.
# Usage: host-package.sh flatpak flatpak run --command=pkd io.github.astrovm.PkgDeck
#        host-package.sh direct /path/PkgDeck.AppImage --cli
#        host-package.sh direct snap run pkgdeck.pkd
set -euo pipefail
mode=${1:?Expected flatpak or direct followed by packaged CLI command}
shift
case "$mode" in
    flatpak)
        if [[ ${1:-} != --session ]]; then
            exec dbus-run-session -- bash "$0" flatpak --session "$@"
        fi
        shift
        ;;
    direct) ;;
    *) echo 'Expected flatpak or direct' >&2; exit 2 ;;
esac
[[ $# -gt 0 ]] || { echo 'Packaged CLI command required' >&2; exit 2; }
python3 "$(dirname "$0")/apt-python.py"
fixture=$(mktemp -d /tmp/pkgdeck-host-parity.XXXXXX)
trap 'rm -rf "$fixture"' EXIT
mkdir -p "$fixture/bin" "$fixture/prefix"
cat > "$fixture/bin/brew" <<'BREW'
#!/bin/sh
set -eu
# This fixture must execute on the host, not against the packaged filesystem.
[ ! -e /.flatpak-info ] || { echo 'brew ran in sandbox' >&2; exit 90; }
root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
case "${1:-}" in
    --prefix) printf '%s/prefix\n' "$root" ;;
    formulae) printf 'pkgdeck-synthetic-host\n' ;;
    info)
        installed='[]'
        [ ! -e "$root/installed" ] || installed='[{"version":"1.0"}]'
        if [ "${4:-}" = --installed ] && [ ! -e "$root/installed" ]; then
            printf '{"formulae":[]}\n'
        else
            printf '{"formulae":[{"full_name":"pkgdeck-synthetic-host","desc":"Disposable host bridge fixture","homepage":"","versions":{"stable":"1.0"},"revision":0,"installed":%s,"linked_keg":null,"outdated":false,"dependencies":[]}]}\n' "$installed"
        fi
        ;;
    install)
        [ "$*" = 'install --formula -- pkgdeck-synthetic-host' ] || exit 91
        printf 'installed by packaged CLI\n' > "$root/installed"
        ;;
    uninstall)
        [ "$*" = 'uninstall --formula --force -- pkgdeck-synthetic-host' ] || exit 92
        rm "$root/installed"
        ;;
    *) echo "Unexpected synthetic brew call: $*" >&2; exit 93 ;;
esac
BREW
chmod +x "$fixture/bin/brew"
export PATH="$fixture/bin:$PATH"
if [[ $mode = flatpak ]]; then
    # A private session prevents changing the desktop's activation environment.
    # Flatpak's host bridge discovers this PATH from its host-side service.
    dbus-update-activation-environment PATH
fi
packaged_cli=("$@")
check() {
    local name=$1 filter=$2
    shift 2
    if ! "${packaged_cli[@]}" --json --yes --auth sudo "$@" > "$fixture/$name.json"; then
        cat "$fixture/$name.json"
        return 1
    fi
    jq -e "$filter" "$fixture/$name.json" || { cat "$fixture/$name.json"; return 1; }
}
# No fallback to a native pkd: query the real host package database through
# the installed artifact. bash is installed on the Ubuntu package runners.
check apt-info '.exit_code == 0 and .data.package.id.name == "bash" and .data.package.installed_version != null' --from apt --arch "$(dpkg --print-architecture)" info bash
check brew-search 'any(.data.packages[]; .id.name == "pkgdeck-synthetic-host")' --from homebrew search pkgdeck-synthetic-host
check brew-install '.exit_code == 0' --from homebrew install pkgdeck-synthetic-host
test -f "$fixture/installed"
check brew-list 'any(.data.packages[]; .id.name == "pkgdeck-synthetic-host" and .installed_version == "1.0")' --from homebrew list
check brew-remove '.exit_code == 0' --from homebrew remove pkgdeck-synthetic-host
test ! -e "$fixture/installed"
echo "PASS packaged host APT query and synthetic Homebrew lifecycle ($mode)"
