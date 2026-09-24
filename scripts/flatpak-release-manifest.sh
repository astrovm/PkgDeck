#!/usr/bin/env bash
set -euo pipefail

tag=${1:?Usage: scripts/flatpak-release-manifest.sh TAG OUTPUT}
output=${2:?Usage: scripts/flatpak-release-manifest.sh TAG OUTPUT}
case "$tag" in v*) ;; *) echo "tag must start with v" >&2; exit 2 ;; esac

url="https://github.com/astrovm/PkgDeck/archive/refs/tags/$tag.tar.gz"
archive=$(mktemp)
trap 'rm -f "$archive"' EXIT
curl -fL --retry 3 "$url" -o "$archive"
sha=$(sha256sum "$archive" | cut -d' ' -f1)

awk -v url="$url" -v sha="$sha" '
  /^      - type: dir$/ {
    print "      - type: archive"
    print "        url: " url
    print "        sha256: " sha
    getline
    next
  }
  { print }
' packaging/flatpak/io.github.astrovm.PkgDeck.yml > "$output"
