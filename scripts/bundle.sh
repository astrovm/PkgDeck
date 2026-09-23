#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."
out=build/AppDir
rm -rf -- "$out"
mkdir -p "$out/usr/"{bin,lib,qml,plugins}
case "$(uname -m)" in
    x86_64)
        updater_url='https://github.com/AppImageCommunity/AppImageUpdate/releases/download/2.0.0-alpha-1-20251018/appimageupdatetool-x86_64.AppImage'
        updater_sha256='d976cdac667b03dee8cb23fb95ef74b042c406c5cbab3ff294d2b16efeaff84f'
        ;;
    aarch64)
        updater_url='https://github.com/AppImageCommunity/AppImageUpdate/releases/download/2.0.0-alpha-1-20251018/appimageupdatetool-aarch64.AppImage'
        updater_sha256='7aaf89dd4cf66ebd940d416c67e1c240c57a139cee38d9c0ed3bb9387bc435b0'
        ;;
    *) echo "Unsupported AppImage updater architecture: $(uname -m)" >&2; exit 1 ;;
esac
mkdir -p "$out/usr/lib/pkgdeck"
curl -fL --retry 3 "$updater_url" -o "$out/usr/lib/pkgdeck/appimageupdatetool.AppImage"
printf '%s  %s\n' "$updater_sha256" "$out/usr/lib/pkgdeck/appimageupdatetool.AppImage" | sha256sum --check --status
chmod +x "$out/usr/lib/pkgdeck/appimageupdatetool.AppImage"
scripts/build-apt.sh "${CARGO_TARGET_DIR:-target}/release"
cp "${CARGO_TARGET_DIR:-target}/release/"{pkd,pkgdeck,pkgdeck-apt-query} "$out/usr/bin/"
for module in QtQuick QtQml QtCore QtNetwork org; do cp -a "$QT_QML_DIR/$module" "$out/usr/qml/"; done
mkdir -p "$out/usr/qml/Qt/labs"
cp -a "$QT_QML_DIR/Qt/labs/platform" "$out/usr/qml/Qt/labs/"
shopt -s nullglob
for pattern in platforms/libqoffscreen.so platforms/libqminimal.so platforms/libqxcb.so 'platforms/libqwayland*.so' 'imageformats/libqjpeg.so' 'imageformats/libqico.so' 'imageformats/libqsvg.so' 'imageformats/libqwebp.so' 'tls/*.so' 'iconengines/*.so' 'xcbglintegrations/*.so' 'wayland-graphics-integration-client/*.so' 'wayland-shell-integration/*.so' platforminputcontexts/libcomposeplatforminputcontextplugin.so platforminputcontexts/libibusplatforminputcontextplugin.so; do
    for source in "$QT_PLUGIN_DIR"/$pattern; do
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
LD_LIBRARY_PATH="$(realpath "$out/usr/lib"):$QT_LIB_DIR"
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
