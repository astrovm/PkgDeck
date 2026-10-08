#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."
out=build/AppDir
rm -rf -- "$out"
mkdir -p "$out/usr/"{bin,lib,libexec}
scripts/appimage-updater.sh "$out/usr"
scripts/build-apt.sh "${CARGO_TARGET_DIR:-target}/release"
cp "${CARGO_TARGET_DIR:-target}/release/"{pkd,pkgdeck,pkgdeck-apt-query} "$out/usr/bin/"
cp "${CARGO_TARGET_DIR:-target}/release/pkgdeck-host-runner" "$out/usr/libexec/"
cp packaging/appimage/AppRun "$out/AppRun"
chmod +x "$out/AppRun"
for pair in desktop:applications metainfo.xml:metainfo svg:icons/hicolor/scalable/apps; do
    suffix=${pair%%:*}
    directory=${pair#*:}
    name=io.github.astrovm.PkgDeck.$suffix
    mkdir -p "$out/usr/share/$directory"
    cp "assets/$name" "$out/usr/share/$directory/"
    if [[ $suffix != metainfo.xml ]]; then cp "assets/$name" "$out/"; fi
done
# The window draws with OpenGL and talks to X11 or Wayland through the
# system's own libraries, which it loads at run time; everything the
# binaries link directly is bundled below.
LD_LIBRARY_PATH="$(realpath "$out/usr/lib")"
export LD_LIBRARY_PATH
mapfile -d '' queue < <(find "$out" -type f \( -name '*.so*' -o -name pkd -o -name pkgdeck -o -name pkgdeck-apt-query -o -name pkgdeck-host-runner \) -print0)
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
        [[ $name =~ ^(ld-linux.*|lib(c|m|dl|pthread|rt|resolv|util|anl)\.so\..*|lib(GL|EGL|GLX|GLdispatch|OpenGL)\.so\..*)$ ]] && continue
        if [[ ! -e $out/usr/lib/$name ]]; then
            cp "$source" "$out/usr/lib/$name"
            queue+=("$out/usr/lib/$name")
        fi
    done <<<"$links"
done
mkdir -p "$out/usr/share/licenses/pkgdeck"
cp LICENSE "$out/usr/share/licenses/pkgdeck/"
cp crates/pkgdeck/assets/fonts/Inter-LICENSE.txt "$out/usr/share/licenses/pkgdeck/INTER-OFL"
cp native/COPYING "$out/usr/share/licenses/pkgdeck/APT-HELPER-GPL-2"
apt_library=$(ldd "$out/usr/bin/pkgdeck-apt-query" | awk '/libapt-pkg/{print $1}')
apt_package=$(dpkg-query -S "/usr/lib/$(gcc -dumpmachine)/$apt_library" | head -1 | cut -d: -f1)
cp "/usr/share/doc/$apt_package/copyright" "$out/usr/share/licenses/pkgdeck/APT-COPYRIGHT"
