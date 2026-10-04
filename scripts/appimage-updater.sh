#!/usr/bin/env bash
# Download the pinned AppImage updater every Linux build ships at
# lib/pkgdeck/appimageupdatetool.AppImage, next to bin/. The Flatpak manifest
# pins the same files; scripts/tests/release-packages.sh checks they match.
#   appimage-updater.sh PREFIX   install into PREFIX/lib/pkgdeck
#   appimage-updater.sh --pins   print "arch url sha256" for every architecture
set -euo pipefail
release=https://github.com/AppImageCommunity/AppImageUpdate/releases/download/2.0.0-alpha-1-20251018
pins=(
    "x86_64 $release/appimageupdatetool-x86_64.AppImage d976cdac667b03dee8cb23fb95ef74b042c406c5cbab3ff294d2b16efeaff84f"
    "aarch64 $release/appimageupdatetool-aarch64.AppImage 7aaf89dd4cf66ebd940d416c67e1c240c57a139cee38d9c0ed3bb9387bc435b0"
)
prefix=${1:?Usage: appimage-updater.sh PREFIX|--pins}
if [[ $prefix == --pins ]]; then
    printf '%s\n' "${pins[@]}"
    exit 0
fi
arch=$(uname -m)
for pin in "${pins[@]}"; do
    read -r pin_arch url sha256 <<<"$pin"
    [[ $pin_arch == "$arch" ]] || continue
    destination=$prefix/lib/pkgdeck/appimageupdatetool.AppImage
    mkdir -p "${destination%/*}"
    curl -fL --retry 5 --retry-delay 5 --retry-all-errors "$url" -o "$destination"
    printf '%s  %s\n' "$sha256" "$destination" | sha256sum --check --status
    chmod +x "$destination"
    exit 0
done
echo "Unsupported AppImage updater architecture: $arch" >&2
exit 1
