#!/usr/bin/env bash
# Source this file to select the repository's SDK without changing system packages.
pkgdeck_env() {
    local arch qt_dir linker
    arch=$(uname -m)
    case "$arch" in
        x86_64) qt_dir=gcc_64 ;;
        aarch64) qt_dir=gcc_arm64 ;;
        *) printf 'Unsupported SDK architecture: %s\n' "$arch" >&2; return 1 ;;
    esac
    export PKGDECK_CACHE_DIR="${PKGDECK_CACHE_DIR:-${XDG_CACHE_HOME:-$HOME/.cache}/pkgdeck/$arch}"
    export QT_ROOT_DIR="${QT_ROOT_DIR:-$PKGDECK_CACHE_DIR/Qt/6.11.2/$qt_dir}"
    export PKGDECK_SDK_PREFIX="${PKGDECK_SDK_PREFIX:-$PKGDECK_CACHE_DIR/kde-6.30.0}"
    export PATH="$PKGDECK_CACHE_DIR/tools/bin:$PKGDECK_CACHE_DIR/python/bin:$QT_ROOT_DIR/bin:$PATH"
    if ! command -v ld.lld >/dev/null; then
        for linker in /usr/lib/llvm-*/bin/ld.lld; do
            if [[ -x "$linker" ]]; then export PATH="${linker%/*}:$PATH"; break; fi
        done
    fi
    export CMAKE_PREFIX_PATH="$PKGDECK_SDK_PREFIX:$QT_ROOT_DIR${CMAKE_PREFIX_PATH:+:$CMAKE_PREFIX_PATH}"
    export LD_LIBRARY_PATH="$PKGDECK_SDK_PREFIX/lib:$QT_ROOT_DIR/lib${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
    export QML_IMPORT_PATH="$PKGDECK_SDK_PREFIX/qml:$QT_ROOT_DIR/qml${QML_IMPORT_PATH:+:$QML_IMPORT_PATH}"
}
pkgdeck_env
unset -f pkgdeck_env
