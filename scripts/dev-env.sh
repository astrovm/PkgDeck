#!/usr/bin/env bash
# Source this file to select Ubuntu's Qt and KDE development packages.
pkgdeck_env() {
    local linker qtpaths
    qtpaths=$(command -v qtpaths6 || command -v qtpaths) || { printf 'Missing qtpaths6; install Ubuntu Qt 6 development packages.\n' >&2; return 1; }
    export QT_ROOT_DIR="$($qtpaths --query QT_INSTALL_PREFIX)"
    export QT_BIN_DIR="$($qtpaths --query QT_INSTALL_BINS)"
    export QT_QML_DIR="$($qtpaths --query QT_INSTALL_QML)"
    export QT_PLUGIN_DIR="$($qtpaths --query QT_INSTALL_PLUGINS)"
    export QT_LIB_DIR="$($qtpaths --query QT_INSTALL_LIBS)"
    export PKGDECK_SDK_PREFIX=/usr
    if ! command -v ld.lld >/dev/null; then
        for linker in /usr/lib/llvm-*/bin/ld.lld; do
            if [[ -x "$linker" ]]; then export PATH="${linker%/*}:$PATH"; break; fi
        done
    fi
    export PATH="$QT_BIN_DIR:$PATH"
    export CMAKE_PREFIX_PATH="/usr${CMAKE_PREFIX_PATH:+:$CMAKE_PREFIX_PATH}"
    export QML_IMPORT_PATH="$QT_QML_DIR${QML_IMPORT_PATH:+:$QML_IMPORT_PATH}"
}
pkgdeck_env
unset -f pkgdeck_env
