#!/usr/bin/env bash
set -euo pipefail
arch=$(uname -m)
version=${PKGDECK_VERSION:-$(sed -n 's/^version = "\([^"]*\)"/\1/p' Cargo.toml | head -n1)}
[[ $version =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || { echo "Invalid PkgDeck version: $version" >&2; exit 1; }
name="PkgDeck-v$version-$arch"
mkdir -p build/artifacts
case "${1:?Usage: scripts/package.sh appimage|flatpak|snap}" in
    appimage)
        : "${APPIMAGETOOL:?Set APPIMAGETOOL to the appimagetool executable}"
        update_information="gh-releases-zsync|astrovm|PkgDeck|latest|PkgDeck-v*-$arch.AppImage.zsync"
        ARCH="$arch" "$APPIMAGETOOL" --appimage-extract-and-run \
            -u "$update_information" "$PWD/build/AppDir" "$PWD/build/artifacts/$name.AppImage"
        ;;
    flatpak)
        flatpak-builder --user --force-clean --repo=build/flatpak-repo build/flatpak packaging/flatpak/io.github.astrovm.PkgDeck.yml
        flatpak build-bundle build/flatpak-repo "build/artifacts/$name.flatpak" io.github.astrovm.PkgDeck
        ;;
    snap)
        case "$arch" in x86_64) snap_arch=amd64 ;; aarch64) snap_arch=arm64 ;; *) exit 1 ;; esac
        rm -rf -- build/snap
        mkdir -p build/snap
        cp -a build/AppDir/. build/snap/
        mkdir -p build/snap/meta/gui
        sed -e "s/ARCHITECTURE/$snap_arch/" -e "s/^version: .*/version: '$version'/" \
            packaging/snap/snap.yaml > build/snap/meta/snap.yaml
        cp assets/io.github.astrovm.PkgDeck.svg build/snap/meta/gui/icon.svg
        # shellcheck disable=SC2016
        sed 's|Exec=pkgdeck|Exec=pkgdeck|; s|Icon=io.github.astrovm.PkgDeck|Icon=${SNAP}/meta/gui/icon.svg|' \
            assets/io.github.astrovm.PkgDeck.desktop > build/snap/meta/gui/pkgdeck.desktop
        snap pack build/snap build/artifacts
        mv "build/artifacts/pkgdeck_${version}_${snap_arch}.snap" "build/artifacts/$name.snap"
        ;;
    *) exit 2 ;;
esac
