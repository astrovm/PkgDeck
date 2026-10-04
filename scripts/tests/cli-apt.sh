#!/usr/bin/env bash
# Exercise the same APT runtime included in the Homebrew CLI archive.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/../.."
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
scripts/bundle-cli-apt.sh "$work/original"
mv "$work/original" "$work/relocated prefix"
prefix="$work/relocated prefix"
test -s "$prefix/share/licenses/pkgdeck/apt/APT-HELPER-GPL-2"
test -s "$prefix/share/licenses/pkgdeck/apt/libc6"
PKGDECK_APT_HELPER="$prefix/bin/pkgdeck-apt-query" scripts/tests/apt-metadata.sh
echo 'PASS relocated CLI APT runtime with synthetic package metadata'
