#!/usr/bin/env bash
set -euo pipefail
tree=$(cargo tree --locked -p pkd)
if [[ $tree == *cxx-qt* || $tree == *qt-build* ]]; then
    echo 'Qt dependency leaked into pkd' >&2
    exit 1
fi
binary=${CARGO_TARGET_DIR:-target}/debug/pkd
# Linux lists the shared libraries with ldd, which macOS does not have; there
# otool reads the same information from the load commands.
if [[ $(uname -s) == Darwin ]]; then
    links=$(otool -L "$binary" | tail -n +2)
else
    links=$(ldd "$binary")
fi
if grep -qi 'qt' <<<"$links"; then
    echo 'pkd links Qt' >&2
    exit 1
fi
echo 'Terminal dependency graph and binary are Qt-free'
