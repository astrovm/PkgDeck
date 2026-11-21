#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."
out=build/AppDir
rm -rf -- "$out"
mkdir -p "$out/usr/"{bin,lib,libexec}
# CI builds these in `scripts/container.sh portable` and passes its release
# folder, so they work with glibc 2.35 (docs/distribution.md).
release=${PKGDECK_RELEASE_DIR:-${CARGO_TARGET_DIR:-target}/release}
scripts/appimage-updater.sh "$out/usr"
cp "$release/"{pkd,pkgdeck} "$out/usr/bin/"
cp "$release/pkgdeck-host-runner" "$out/usr/libexec/"
# The APT reader brings its own libraries, glibc and loader, like the CLI
# archive. So it can use a newer APT than the oldest supported system has.
scripts/bundle-apt.sh "$out/usr"
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
queue=("$out/usr/bin/pkd" "$out/usr/bin/pkgdeck" "$out/usr/libexec/pkgdeck-host-runner")
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
cp crates/pkgdeck/assets/fonts/Selawik-LICENSE.txt "$out/usr/share/licenses/pkgdeck/SELAWIK-OFL"
cp crates/pkgdeck/assets/fonts/W95FA-LICENSE.txt "$out/usr/share/licenses/pkgdeck/W95FA-OFL"
