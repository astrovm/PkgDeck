#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."
out=build/AppDir
rm -rf -- "$out"
mkdir -p "$out/usr/"{bin,lib,qml,plugins}
scripts/build-apt.sh "${CARGO_TARGET_DIR:-target}/release"
cp "${CARGO_TARGET_DIR:-target}/release/"{pkd,pkgdeck,pkgdeck-apt-query} "$out/usr/bin/"
for module in QtQuick QtQml QtCore; do cp -a "$QT_ROOT_DIR/qml/$module" "$out/usr/qml/"; done
cp -a "$PKGDECK_SDK_PREFIX/qml/org" "$out/usr/qml/"
shopt -s nullglob
for pattern in platforms/libqoffscreen.so platforms/libqminimal.so platforms/libqxcb.so 'platforms/libqwayland*.so' 'imageformats/libqjpeg.so' 'imageformats/libqsvg.so' 'imageformats/libqico.so' 'iconengines/*.so' 'xcbglintegrations/*.so' 'wayland-graphics-integration-client/*.so' 'wayland-shell-integration/*.so' platforminputcontexts/libcomposeplatforminputcontextplugin.so platforminputcontexts/libibusplatforminputcontextplugin.so; do
    for source in "$QT_ROOT_DIR"/plugins/$pattern; do
        mkdir -p "$out/usr/plugins/${pattern%/*}"
        cp "$source" "$out/usr/plugins/${pattern%/*}/"
    done
done
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
printf '[Paths]\nPrefix=..\nLibraries=lib\nPlugins=plugins\nQmlImports=qml\n' >"$out/usr/bin/qt.conf"
LD_LIBRARY_PATH="$(realpath "$out/usr/lib"):$QT_ROOT_DIR/lib:$PKGDECK_SDK_PREFIX/lib"
export LD_LIBRARY_PATH
mapfile -d '' queue < <(find "$out" -type f \( -name '*.so*' -o -name pkd -o -name pkgdeck -o -name pkgdeck-apt-query \) -print0)
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
cp LICENSE crates/pkgdeck/assets/OCTICONS-LICENSE "$out/usr/share/licenses/pkgdeck/"
cp native/COPYING "$out/usr/share/licenses/pkgdeck/APT-HELPER-GPL-2"
apt_library=$(ldd "$out/usr/bin/pkgdeck-apt-query" | awk '/libapt-pkg/{print $1}')
apt_package=$(dpkg-query -S "/usr/lib/$(gcc -dumpmachine)/$apt_library" | head -1 | cut -d: -f1)
cp "/usr/share/doc/$apt_package/copyright" "$out/usr/share/licenses/pkgdeck/APT-COPYRIGHT"
