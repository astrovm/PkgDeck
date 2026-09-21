#!/usr/bin/env bash
# Isolated HTTPS/WebP fixture. Optional argument selects staged Qt libraries/plugins.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/../.."
fixture=$(mktemp -d)
trap 'rm -rf "$fixture"' EXIT
openssl req -x509 -newkey rsa:2048 -nodes -days 1 -subj /CN=localhost \
    -addext subjectAltName=DNS:localhost -keyout "$fixture/key.pem" -out "$fixture/cert.pem" >"$fixture/openssl.log" 2>&1
read -r -a qt_flags <<<"$(pkg-config --cflags --libs Qt6Quick Qt6Qml Qt6Network Qt6Gui)"
c++ -std=c++17 -fPIC crates/pkgdeck/tests/media-cache.cpp crates/pkgdeck/native/network.cpp \
    "${qt_flags[@]}" -o "$fixture/media-cache"
if [[ $# -gt 0 ]]; then
    bundle=$(realpath "$1")
    printf '[Paths]\nPrefix=%s\nLibraries=lib\nPlugins=plugins\nQmlImports=qml\n' "$bundle" >"$fixture/qt.conf"
    export LD_LIBRARY_PATH="$bundle/lib"
    export QT_PLUGIN_PATH="$bundle/plugins"
    export QML_IMPORT_PATH="$bundle/qml"
    unset QML2_IMPORT_PATH
fi
export QT_QPA_PLATFORM=offscreen QT_QUICK_BACKEND=software
export XDG_CACHE_HOME="$fixture/cache" XDG_CONFIG_HOME="$fixture/config"
export XDG_RUNTIME_DIR="$fixture/runtime"
mkdir -m 700 "$XDG_RUNTIME_DIR"
timeout 40s "$fixture/media-cache" "$fixture" seed
timeout 40s "$fixture/media-cache" "$fixture" reuse
