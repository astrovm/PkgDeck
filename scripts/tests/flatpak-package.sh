#!/usr/bin/env bash
# Exercise the installed package's host bridge inside a real desktop session bus.
set -euo pipefail
if [[ ${1:-} != --session ]]; then
    exec dbus-run-session -- bash "$0" --session
fi
# A separate private bus lets this test supply a disposable host manager without
# contaminating the real Flatpak lifecycle's host environment.
bash "$(dirname "$0")/host-package.sh" flatpak flatpak run --command=pkd io.github.astrovm.PkgDeck
log_dir=$(mktemp -d)
trap 'rm -rf "$log_dir"' EXIT
flatpak --user remote-add --if-not-exists flathub https://flathub.org/repo/flathub.flatpakrepo
flatpak --user install -y --noninteractive flathub org.kde.kcalc
run_flatpak() {
    flatpak run --command=pkd io.github.astrovm.PkgDeck --json --yes --auth sudo --from flatpak "$@"
}
check() {
    local name=$1 filter=$2
    shift 2
    if ! "$@" >"$log_dir/$name.json"; then
        cat "$log_dir/$name.json"
        return 1
    fi
    jq -e "$filter" "$log_dir/$name.json" || { cat "$log_dir/$name.json"; return 1; }
}
check list 'any(.data.packages[]; .id.name == "org.kde.kcalc")' run_flatpak list
check info '.exit_code == 0' run_flatpak --scope user info org.kde.kcalc
check update '.exit_code == 0' run_flatpak update
check remove '.exit_code == 0' run_flatpak --scope user remove org.kde.kcalc
! flatpak --user list --app | grep -q org.kde.kcalc
