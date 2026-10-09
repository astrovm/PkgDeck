#!/usr/bin/env bash
# Neither the window nor the terminal tool depends on Qt any more; keep it
# that way, in the dependency graph and in what pkd links.
set -euo pipefail
tree=$(cargo tree --locked --workspace)
if [[ $tree == *cxx-qt* || $tree == *qt-build* ]]; then
    echo 'Qt dependency leaked into the workspace' >&2
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
echo 'Dependency graph and pkd are Qt-free'
