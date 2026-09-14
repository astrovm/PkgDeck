#!/usr/bin/env bash
set -euo pipefail
tree=$(cargo tree --locked -p pkd)
if [[ $tree == *cxx-qt* || $tree == *qt-build* ]]; then
    echo 'Qt dependency leaked into pkd' >&2
    exit 1
fi
links=$(ldd "${CARGO_TARGET_DIR:-target}/debug/pkd")
if [[ ${links,,} == *qt* ]]; then
    echo 'pkd links Qt' >&2
    exit 1
fi
echo 'Terminal dependency graph and binary are Qt-free'
