#!/usr/bin/env bash
# Source this file to put the LLVM linker on PATH when Ubuntu installs it
# under a versioned folder.
pkgdeck_env() {
    local linker
    if ! command -v ld.lld >/dev/null; then
        for linker in /usr/lib/llvm-*/bin/ld.lld; do
            if [[ -x "$linker" ]]; then export PATH="${linker%/*}:$PATH"; break; fi
        done
    fi
}
pkgdeck_env
unset -f pkgdeck_env
