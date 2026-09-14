#!/usr/bin/env bash
# Called under setup-dev.sh's cache lock. Only verified downloads are extracted.
set -euo pipefail
source scripts/dev-env.sh
arch=$(uname -m)
archives="$PKGDECK_CACHE_DIR/archives"
mkdir -p "$archives"
fetch() {
    local sha=$1 url=$2 file="$archives/${2##*/}"
    if [[ ! -f $file ]] || ! printf '%s  %s\n' "$sha" "$file" | sha256sum -c --status; then
        curl -fL --retry 3 --max-time 600 "$url" -o "$file.part"
        printf '%s  %s\n' "$sha" "$file.part" | sha256sum -c -
        mv "$file.part" "$file"
    fi
}
cmake_name=cmake-4.4.3-linux-$arch
sha=$(awk -v file="$cmake_name.tar.gz" '$2==file{print $1}' scripts/sdk.sha256)
[[ $sha =~ ^[0-9a-f]{64}$ ]]
if [[ ! -f $PKGDECK_CACHE_DIR/$cmake_name/.pkgdeck-complete ]] || [[ $(cat "$PKGDECK_CACHE_DIR/$cmake_name/.pkgdeck-complete") != "$sha" ]]; then
    fetch "$sha" "https://github.com/Kitware/CMake/releases/download/v4.4.3/$cmake_name.tar.gz"
    staging=$(mktemp -d "$PKGDECK_CACHE_DIR/cmake-stage-XXXXXX")
    trap 'rm -rf -- "$staging"' EXIT
    tar -xzf "$archives/$cmake_name.tar.gz" -C "$staging"
    "$staging/$cmake_name/bin/cmake" --version
    printf '%s\n' "$sha" >"$staging/$cmake_name/.pkgdeck-complete"
    rm -rf -- "${PKGDECK_CACHE_DIR:?}/$cmake_name"
    mv "$staging/$cmake_name" "$PKGDECK_CACHE_DIR/"
    rm -rf -- "$staging"
    trap - EXIT
fi
key=$(sha256sum scripts/qt-archives.tsv | cut -d ' ' -f1)
if [[ ! -f $QT_ROOT_DIR/.pkgdeck-archives ]] || [[ $(cat "$QT_ROOT_DIR/.pkgdeck-archives") != "$key" ]]; then
    staging=$(mktemp -d "$PKGDECK_CACHE_DIR/qt-stage-XXXXXX")
    trap 'rm -rf -- "$staging"' EXIT
    count=0
    while IFS=$'\t' read -r platform destination sha url; do
        [[ $platform == "$arch" ]] || continue
        fetch "$sha" "$url"
        mkdir -p "$staging/$destination"
        bsdtar -xf "$archives/${url##*/}" -C "$staging/$destination"
        count=$((count + 1))
    done <scripts/qt-archives.tsv
    [[ $count -gt 0 ]]
    # Qt's binary SDK is relocatable through qt.conf, also used by qmake/qtpaths.
    printf '[Paths]\nPrefix=..\n' >"$staging/bin/qt.conf"
    [[ $("$staging/bin/qtpaths" --qt-version) == 6.11.2 ]]
    printf '%s\n' "$key" >"$staging/.pkgdeck-archives"
    # Container bootstrap runs as root; the SDK must be usable by developers.
    chmod 755 "$staging"
    mkdir -p "$(dirname "$QT_ROOT_DIR")"
    rm -rf -- "$QT_ROOT_DIR"
    mv "$staging" "$QT_ROOT_DIR"
    trap - EXIT
fi
