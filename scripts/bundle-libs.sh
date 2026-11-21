#!/usr/bin/env bash
# Copy the shared libraries that programs need into a folder, and the
# libraries those need in turn. The word `window` stands for the X11
# libraries that the window (winit) opens at run time: ldd can't see them,
# and minimal systems, such as the Ubuntu 22.04 the AppImage catalog tests
# on, don't have them.
#
# Libraries on the AppImage exclude list stay out: every system has them, and
# glibc, OpenGL, libX11 and libxcb must match the system's own glibc, graphics
# drivers and display server. The AppImage catalog warns about bundled ones.
# https://github.com/AppImage/pkg2appimage/blob/master/excludelist
#
# Usage: scripts/bundle-libs.sh <folder> (window | <program or library>)...
set -euo pipefail
usage='Usage: scripts/bundle-libs.sh <folder> (window | <program or library>)...'
dest=${1:?$usage}
shift
(($#)) || { echo "$usage" >&2; exit 2; }
window_libraries=(libxkbcommon.so.0 libxkbcommon-x11.so.0 libXcursor.so.1 libXi.so.6)
excluded='^(ld-linux.*|lib(c|m|dl|pthread|rt|resolv|util|anl)\.so\..*|lib(GL|EGL|GLX|GLdispatch|OpenGL)\.so\..*|lib(X11|X11-xcb|xcb|gcc_s)\.so\..*)$'
mkdir -p "$dest"
dest=$(realpath "$dest")
# ldd finds the libraries copied so far first, so it reports what is still
# missing from the folder, not from this system.
export LD_LIBRARY_PATH="$dest${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
queue=()
for item in "$@"; do
    if [[ $item != window ]]; then
        queue+=("$item")
        continue
    fi
    for name in "${window_libraries[@]}"; do
        path=$(/sbin/ldconfig -p | awk -v name="$name" '$1 == name { print $NF; exit }')
        [[ -n $path ]] || { echo "Missing $name on this system" >&2; exit 1; }
        cp -L "$path" "$dest/$name"
        queue+=("$dest/$name")
    done
done
for ((i = 0; i < ${#queue[@]}; i++)); do
    path=${queue[i]}
    links=$(ldd "$path") || {
        echo "Cannot inspect $path" >&2
        exit 1
    }
    if [[ $links == *'not found'* ]]; then
        echo "Unresolved dependency in $path: $links" >&2
        exit 1
    fi
    while read -r name arrow source rest; do
        [[ $arrow == '=>' && $source == /* ]] || continue
        [[ $name =~ $excluded ]] && continue
        if [[ ! -e $dest/$name ]]; then
            cp -L "$source" "$dest/$name"
            queue+=("$dest/$name")
        fi
    done <<<"$links"
done
