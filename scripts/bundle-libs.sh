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
#
# Usage: scripts/bundle-libs.sh <folder> (window | <program or library>)...
set -euo pipefail
usage='Usage: scripts/bundle-libs.sh <folder> (window | <program or library>)...'
dest=${1:?$usage}
shift
(($#)) || { echo "$usage" >&2; exit 2; }
window_libraries=(libxkbcommon.so.0 libxkbcommon-x11.so.0 libXcursor.so.1 libXi.so.6)
# The exclude list the AppImage catalog checks, from
# https://github.com/AppImage/AppImages/blob/master/excludelist (53 names,
# November 2026), plus the aarch64 loader, which the list does not name.
excluded_names=(
    ld-linux.so.2 ld-linux-x86-64.so.2 ld-linux-aarch64.so.1
    libanl.so.1 libBrokenLocale.so.1 libcidn.so.1 libc.so.6 libdl.so.2 libm.so.6
    libmvec.so.1 libnss_compat.so.2 libnss_dns.so.2 libnss_files.so.2
    libnss_hesiod.so.2 libnss_nisplus.so.2 libnss_nis.so.2 libpthread.so.0
    libresolv.so.2 librt.so.1 libthread_db.so.1 libutil.so.1 libstdc++.so.6
    libGL.so.1 libEGL.so.1 libGLdispatch.so.0 libGLX.so.0 libOpenGL.so.0
    libdrm.so.2 libglapi.so.0 libgbm.so.1 libxcb.so.1 libX11.so.6
    libX11-xcb.so.1 libwayland-client.so.0 libasound.so.2 libfontconfig.so.1
    libfreetype.so.6 libharfbuzz.so.0 libcom_err.so.2 libexpat.so.1
    libgcc_s.so.1 libgpg-error.so.0 libICE.so.6 libSM.so.6 libusb-1.0.so.0
    libuuid.so.1 libz.so.1 libjack.so.0 libpipewire-0.3.so.0 libxcb-dri3.so.0
    libxcb-dri2.so.0 libfribidi.so.0 libgmp.so.10
)
declare -A excluded
for name in "${excluded_names[@]}"; do
    excluded[$name]=1
done
case $(uname -m) in
    x86_64) abi=x86-64 ;;
    aarch64) abi=AArch64 ;;
    *) echo "Unsupported architecture $(uname -m)" >&2; exit 1 ;;
esac
mkdir -p "$dest"
dest=$(realpath "$dest")
# ldd finds the libraries copied so far first, so it reports what is still
# missing from the folder, not from this system.
export LD_LIBRARY_PATH="$dest${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
# Users other than root often lack the sbin folders in PATH.
export PATH="$PATH:/sbin:/usr/sbin"
queue=()
for item in "$@"; do
    if [[ $item != window ]]; then
        queue+=("$item")
        continue
    fi
    # Read the whole cache first: awk stops at the first match, and a pipe
    # from ldconfig would then end it with SIGPIPE on a large cache.
    cache=$(ldconfig -p)
    for name in "${window_libraries[@]}"; do
        # Only this architecture's copy: a multiarch system also has 32-bit ones.
        path=$(awk -v name="$name" -v abi="$abi" '$1 == name && index($0, "(libc6," abi) { print $NF; exit }' <<<"$cache")
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
        [[ -n ${excluded[$name]:-} ]] && continue
        if [[ ! -e $dest/$name ]]; then
            cp -L "$source" "$dest/$name"
            queue+=("$dest/$name")
        fi
    done <<<"$links"
done
