#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."
out="${1:-${CARGO_TARGET_DIR:-target}/debug}"
mkdir -p "$out"
# APT is deliberately a separate executable: pkd and pkgdeck remain portable.
read -r -a cpp_flags <<<"${CPPFLAGS:-}"
read -r -a ld_flags <<<"${LDFLAGS:-}"
# RPATH covers the private dependency closure after Host strips LD_LIBRARY_PATH.
# shellcheck disable=SC2016
# `[@]+` keeps empty flags arrays from being unbound variables under set -u
# on the default macOS bash 3.2.
"${CXX:-c++}" -std=c++17 -O2 -Wall -Wextra -Werror ${cpp_flags[@]+"${cpp_flags[@]}"} native/apt-query.cpp \
    ${ld_flags[@]+"${ld_flags[@]}"} "${PKGDECK_APT_LIB:--lapt-pkg}" -Wl,--disable-new-dtags,-rpath,'$ORIGIN/../lib' -o "$out/pkgdeck-apt-query"
if [[ $out == */debug ]]; then
    mkdir -p "$out/deps"
    cp "$out/pkgdeck-apt-query" "$out/deps/pkgdeck-apt-query"
fi
