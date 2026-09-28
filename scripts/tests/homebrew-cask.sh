#!/usr/bin/env bash
# Install the app this job just packaged through a Homebrew cask, check it
# starts, and uninstall it. Runs in the macOS packaging job so the app is not
# rebuilt or downloaded again. Checksums of packages other jobs build are
# placeholders: this Mac only installs its own.
# Usage: scripts/tests/homebrew-cask.sh <folder with PkgDeck-v*-macos-*.zip>
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/../.."
[[ ${GITHUB_ACTIONS:-} == true && ${RUNNER_ENVIRONMENT:-} == github-hosted && ${RUNNER_OS:-} == macOS ]] || {
    echo 'Requires the disposable PkgDeck GitHub-hosted macOS runner.' >&2
    exit 1
}
dist=$(cd "${1:?Usage: homebrew-cask.sh <package folder>}" && pwd)
trap 'echo "homebrew-cask FAILED at line $LINENO: $BASH_COMMAND" >&2' ERR
version=$(sed -n 's/^version = "\([^"]*\)"/\1/p' Cargo.toml | head -n1)
brew tap-new --no-git astrovm/pkgdeck
# As the install docs say: Homebrew 6+ only loads trusted third-party taps.
brew trust astrovm/pkgdeck
cleanup() {
    set +e
    brew uninstall --cask astrovm/pkgdeck/pkgdeck 2>/dev/null
    brew untap astrovm/pkgdeck
}
trap cleanup EXIT
tap=$(brew --repository astrovm/pkgdeck)
(cd "$dist" && shasum -a 256 PkgDeck-v*-macos-*.zip > SHA256SUMS)
PKGDECK_RENDER_PLACEHOLDERS=1 scripts/homebrew-render.sh "$version" "$dist/SHA256SUMS" "file://$dist" "$tap"
brew style --cask astrovm/pkgdeck/pkgdeck
# The unqualified name must reach the cask, not the Linux-only formula.
brew install astrovm/pkgdeck/pkgdeck
brew list --cask astrovm/pkgdeck/pkgdeck
if xattr -p com.apple.quarantine /Applications/PkgDeck.app 2>/dev/null; then
    echo 'Installed app is still quarantined' >&2
    exit 1
fi
"$(brew --prefix)/bin/pkd" --version | grep -F pkd
bash scripts/tests/macos-gui.sh "$(brew --prefix)/bin/pkgdeck"
brew uninstall --cask astrovm/pkgdeck/pkgdeck
test ! -e /Applications/PkgDeck.app
test ! -e "$(brew --prefix)/bin/pkd"
echo 'PASS Homebrew cask install, Cocoa startup and uninstall'
