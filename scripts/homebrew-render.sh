#!/usr/bin/env bash
# Render the tap's formula and cask for a release from its SHA256SUMS file.
# CI passes a local URL base and tap directory to test unpublished packages.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."
usage='Usage: homebrew-render.sh VERSION SHA256SUMS [URL_BASE] [TAP_DIR]'
version=${1:?$usage}
sums=${2:?$usage}
base=${3:-}
tap=${4:-$PWD}
[[ $version =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || { echo "Invalid PkgDeck version: $version" >&2; exit 1; }
substitutions=(-e "s|@VERSION@|$version|g")
for pair in LINUX_AARCH64:linux-aarch64-cli.tar.gz LINUX_X86_64:linux-x86_64-cli.tar.gz \
    MACOS_AARCH64:macos-aarch64.zip MACOS_X86_64:macos-x86_64.zip; do
    name="PkgDeck-v$version-${pair#*:}"
    sum=$(awk -v name="$name" '$2 == name || $2 == "*" name {print $1}' "$sums")
    [[ $sum =~ ^[0-9a-f]{64}$ ]] || { echo "Missing checksum for $name" >&2; exit 1; }
    substitutions+=(-e "s|@SHA256_${pair%%:*}@|$sum|")
done
# shellcheck disable=SC2016 # Ruby interpolation, not shell expansion.
[[ -z $base ]] || substitutions+=(-e "s|https://github.com/astrovm/PkgDeck/releases/download/v#{version}|$base|")
mkdir -p "$tap/Formula" "$tap/Casks"
sed "${substitutions[@]}" packaging/homebrew/pkd.rb > "$tap/Formula/pkd.rb"
sed "${substitutions[@]}" packaging/homebrew/pkgdeck.cask.rb > "$tap/Casks/pkgdeck.rb"
# The formula was named pkgdeck while it also built the macOS app; renaming it
# lets `brew install astrovm/pkgdeck/pkgdeck` resolve to the cask on macOS.
rm -f "$tap/Formula/pkgdeck.rb"
printf '{\n  "pkgdeck": "pkd"\n}\n' > "$tap/formula_renames.json"
