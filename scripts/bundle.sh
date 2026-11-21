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
# system's own libraries, which it loads at run time. The X11 keyboard and
# cursor libraries are not on every system, so the AppImage brings them, from
# the Ubuntu 22.04 build when CI made one. Everything the binaries link
# directly is bundled too.
if [[ -d $release/runtime-libs ]]; then
    mkdir -p "$out/usr/lib"
    cp -a "$release/runtime-libs/." "$out/usr/lib/"
else
    scripts/bundle-libs.sh "$out/usr/lib" window
fi
scripts/bundle-libs.sh "$out/usr/lib" "$out/usr/bin/pkd" "$out/usr/bin/pkgdeck" "$out/usr/libexec/pkgdeck-host-runner"
# File managers and the AppImage catalog show .DirIcon as the thumbnail, and
# the catalog wants a PNG. appimagetool links it to the SVG, unless it exists.
icon=usr/share/icons/hicolor/256x256/apps/io.github.astrovm.PkgDeck.png
mkdir -p "$out/${icon%/*}"
rsvg-convert -w 256 -h 256 -o "$out/$icon" assets/io.github.astrovm.PkgDeck.svg
ln -sfn "$icon" "$out/.DirIcon"
mkdir -p "$out/usr/share/licenses/pkgdeck"
cp LICENSE "$out/usr/share/licenses/pkgdeck/"
cp crates/pkgdeck/assets/fonts/Inter-LICENSE.txt "$out/usr/share/licenses/pkgdeck/INTER-OFL"
cp crates/pkgdeck/assets/fonts/Selawik-LICENSE.txt "$out/usr/share/licenses/pkgdeck/SELAWIK-OFL"
cp crates/pkgdeck/assets/fonts/W95FA-LICENSE.txt "$out/usr/share/licenses/pkgdeck/W95FA-OFL"
