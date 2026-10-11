#!/usr/bin/env bash
# Measures how fast PkgDeck starts and how much memory it holds: in the
# tray only, with the window open, and in the tray after the window closed.
# Runs a release build against an empty profile, so your settings and
# caches are left alone. Linux only: it reads /proc and the tray's D-Bus.
#
#   scripts/memory.sh [path/to/pkgdeck]
set -euo pipefail

bin=${1:-target/release/pkgdeck}
[[ -x $bin ]] || { echo "No PkgDeck at $bin; build it with cargo build --release -p pkgdeck" >&2; exit 1; }
command -v busctl >/dev/null || { echo "busctl is needed to open and close the window" >&2; exit 1; }

profile=$(mktemp -d /tmp/pkgdeck-memory.XXXX)
pid=
cleanup() {
  [[ -n $pid ]] && kill "$pid" 2>/dev/null && wait "$pid" 2>/dev/null
  rm -rf "$profile"
}
trap cleanup EXIT
mkdir -p "$profile/config/pkgdeck" "$profile/cache" "$profile/data" "$profile/run"
chmod 700 "$profile/run"
# Past the first-run questions.
echo '{"setupAsked":true}' >"$profile/config/pkgdeck/window.json"
# The window still reaches the desktop with its own runtime folder.
if [[ -n ${WAYLAND_DISPLAY:-} && $WAYLAND_DISPLAY != /* && -n ${XDG_RUNTIME_DIR:-} ]]; then
  export WAYLAND_DISPLAY=$XDG_RUNTIME_DIR/$WAYLAND_DISPLAY
fi
run() {
  XDG_CONFIG_HOME=$profile/config XDG_CACHE_HOME=$profile/cache XDG_DATA_HOME=$profile/data \
    XDG_RUNTIME_DIR=$profile/run "$bin" "$@"
}
pss() {
  awk '/^Pss:/{p=$2} /^Pss_Anon:/{a=$2} /^Pss_File:/{f=$2}
    END{printf "%d MB (anon %d, file %d)\n", p/1024, a/1024, f/1024}' "/proc/$1/smaps_rollup"
}
threads() { awk '/^Threads:/{print $2}' "/proc/$1/status"; }
# Opens or closes the window, as a click on the tray icon does.
toggle() {
  local service
  service=$(busctl --user list --no-legend | awk -v p="StatusNotifierItem-$1-" 'index($1, p) {print $1; exit}')
  [[ -n $service ]] || { echo "The tray icon isn't on D-Bus" >&2; exit 1; }
  busctl --user call "$service" /StatusNotifierItem org.kde.StatusNotifierItem Activate ii 0 0
}

echo "Start to the third frame, 5 runs:"
for _ in 1 2 3 4 5; do
  start=$(date +%s%N)
  run --smoke-test 2>/dev/null | grep -m1 -q PKGDECK_GUI_READY
  echo "  $(( ($(date +%s%N) - start) / 1000000 )) ms"
done

run --background >/dev/null 2>&1 &
pid=$!
sleep 20
echo "Tray only:          $(pss "$pid"), $(threads "$pid") threads"
toggle "$pid"
sleep 20
echo "Window open:        $(pss "$pid"), $(threads "$pid") threads"
toggle "$pid"
sleep 20
echo "Tray, after window: $(pss "$pid"), $(threads "$pid") threads"
