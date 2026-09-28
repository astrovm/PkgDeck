#!/usr/bin/env bash
# Hand a copy of an app someone installed themselves to real Homebrew through
# pkd, with a fixture app from a local tap. Refused checks must change
# nothing; a successful adoption must leave the very same app in place.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/../.."
[[ ${GITHUB_ACTIONS:-} == true && ${GITHUB_REPOSITORY:-} == astrovm/PkgDeck &&
   ${RUNNER_ENVIRONMENT:-} == github-hosted && ${RUNNER_OS:-} == macOS &&
   $EUID != 0 && -n ${GITHUB_WORKSPACE:-} &&
   $(cd "$GITHUB_WORKSPACE" && pwd -P) == "$(pwd -P)" ]] || {
    echo 'Requires the disposable PkgDeck GitHub-hosted macOS runner.' >&2
    exit 1
}
pkd="$PWD/target/debug/pkd"
trap 'echo "macos-adoption FAILED at line $LINENO: $BASH_COMMAND" >&2' ERR
[[ -x $pkd ]]
# The macOS jobs put GNU coreutils first on PATH; BSD stat reads inodes.
# Only debug builds know this fixture's cask (see backends/adopt.rs).
tap=pkgdeck/fixtures
cask="$tap/pkgdeck-adopt-fixture"
failing="$tap/pkgdeck-adopt-failure"
app='/Applications/PkgDeck Adopt Fixture.app'
command_link="$(brew --prefix)/bin/pkgdeck-adopt-fixture"
backups="$HOME/Library/Application Support/PkgDeck/Adoption backups"
work=$(mktemp -d "$RUNNER_TEMP/pkgdeck-adopt.XXXXXX")
for path in "$app" "$command_link"; do
    [[ ! -e $path && ! -L $path ]] || { echo "Fixture already exists: $path" >&2; exit 1; }
done
cleanup() {
    set +e
    brew uninstall --cask "$cask"
    brew uninstall --cask "$failing"
    brew untap "$tap"
    rm -rf "$app" "$work"
    if [[ -L $command_link || -f $command_link ]]; then rm -f "$command_link"; fi
}
trap cleanup EXIT

# A real Mach-O executable, so the architecture check reads it with lipo.
bundle="$work/PkgDeck Adopt Fixture.app"
mkdir -p "$bundle/Contents/MacOS" "$bundle/Contents/Resources"
printf 'int main(void) { return 0; }\n' | clang -x c - -o "$bundle/Contents/MacOS/fixture"
printf '#!/bin/sh\necho adopted\n' > "$bundle/Contents/Resources/pkgdeck-adopt-fixture"
chmod 755 "$bundle/Contents/Resources/pkgdeck-adopt-fixture"
cat > "$bundle/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleIdentifier</key><string>io.github.astrovm.pkgdeck.adopt-fixture</string>
<key>CFBundleName</key><string>PkgDeck Adopt Fixture</string>
<key>CFBundleExecutable</key><string>fixture</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>CFBundleShortVersionString</key><string>1.2.3</string>
<key>CFBundleVersion</key><string>1</string>
</dict></plist>
EOF
codesign --force --sign - "$bundle"
ditto -c -k --keepParent "$bundle" "$work/fixture.zip"
digest=$(shasum -a 256 "$work/fixture.zip" | awk '{print $1}')
brew tap-new --no-git "$tap"
recipe="$(brew --repository "$tap")/Casks/pkgdeck-adopt-fixture.rb"
mkdir -p "$(dirname "$recipe")"
cat > "$recipe" <<EOF
cask "pkgdeck-adopt-fixture" do
  version "1.2.3"
  sha256 "$digest"
  url "file://$work/fixture.zip"
  name "PkgDeck Adopt Fixture"
  desc "Disposable adoption test fixture"
  homepage "https://github.com/astrovm/PkgDeck"
  auto_updates true
  app "PkgDeck Adopt Fixture.app"
  binary "#{appdir}/PkgDeck Adopt Fixture.app/Contents/Resources/pkgdeck-adopt-fixture"
end
EOF

# The copy someone installed themselves.
ditto "$bundle" "$app"
inode=$(/usr/bin/stat -f %i "$app")
install() { "$pkd" --json --yes --from homebrew-cask install "$cask"; }
installed() { brew list --cask --versions "$cask" >/dev/null 2>&1; }

# 1. A command link that belongs to something else: refused, nothing changes.
printf '#!/bin/sh\n' > "$command_link"
output=$(install) || true
grep -q 'belongs to something else' <<<"$output" || { echo "Expected a refusal: $output" >&2; exit 1; }
if installed; then echo "Refused adoption still installed the cask" >&2; exit 1; fi
[[ $(/usr/bin/stat -f %i "$app") == "$inode" ]]
rm "$command_link"

# 2. A different publisher (here, an unsigned copy): refused before any backup.
codesign --remove-signature "$app"
output=$(install) || true
grep -q "signature doesn't verify\|signed by" <<<"$output" || { echo "Expected a signature refusal: $output" >&2; exit 1; }
if installed; then echo "Refused adoption still installed the cask" >&2; exit 1; fi
codesign --force --sign - "$app"
inode=$(/usr/bin/stat -f %i "$app")

# 3. Checked adoption: Homebrew manages the same folder, links the command,
# and the temporary copy is gone.
output=$(install)
grep -q '"exit_code":0' <<<"$output" || { echo "Adoption failed: $output" >&2; exit 1; }
installed
[[ $(/usr/bin/stat -f %i "$app") == "$inode" ]] || { echo 'The app was replaced instead of adopted' >&2; exit 1; }
[[ $("$command_link") == adopted ]]
[[ ! -d $backups ]] || [[ -z $(ls -A "$backups") ]] || { echo "Backup left behind: $(ls "$backups")" >&2; exit 1; }
# The inventory now names the cask as the owner of this exact copy.
"$pkd" --json --from macos-apps info "$app" |
    jq -e --arg cask "$cask" '.data.description | contains("Managed by Homebrew (\($cask))")' >/dev/null

# 4. Once Homebrew owns it, removing the cask removes the app, as for any cask.
"$pkd" --json --yes --from homebrew-cask remove "$cask" | grep -q '"exit_code":0'
[[ ! -e $app ]]

# 5. Recovery: this cask fails in postflight, after Homebrew took the app.
# Homebrew's rollback deletes the adopted app; PkgDeck must put it back with
# its permissions and extended attributes, and leave no copy behind.
cat > "$(brew --repository "$tap")/Casks/pkgdeck-adopt-failure.rb" <<EOF
cask "pkgdeck-adopt-failure" do
  version "1.2.3"
  sha256 "$digest"
  url "file://$work/fixture.zip"
  name "PkgDeck Adopt Failure"
  desc "Disposable adoption recovery fixture"
  homepage "https://github.com/astrovm/PkgDeck"
  auto_updates true
  app "PkgDeck Adopt Fixture.app"
  postflight do
    raise "PkgDeck recovery test: failing after the app was adopted"
  end
end
EOF
ditto "$bundle" "$app"
xattr -w io.github.astrovm.pkgdeck.test kept "$app"
chmod 700 "$app/Contents/Resources/pkgdeck-adopt-fixture"
output=$("$pkd" --json --yes --from homebrew-cask install "$failing") || true
grep -q 'PkgDeck put it back' <<<"$output" || { echo "Expected a restored app: $output" >&2; exit 1; }
[[ -d $app ]] || { echo 'The app is gone after the failed adoption' >&2; exit 1; }
[[ $(/usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' "$app/Contents/Info.plist") == io.github.astrovm.pkgdeck.adopt-fixture ]]
[[ $(xattr -p io.github.astrovm.pkgdeck.test "$app") == kept ]]
[[ $(/usr/bin/stat -f %Lp "$app/Contents/Resources/pkgdeck-adopt-fixture") == 700 ]]
cmp "$bundle/Contents/MacOS/fixture" "$app/Contents/MacOS/fixture"
if brew list --cask --versions "$failing" >/dev/null 2>&1; then echo 'The failed cask is installed' >&2; exit 1; fi
[[ ! -d $backups ]] || [[ -z $(ls -A "$backups") ]] || { echo "Backup left behind: $(ls "$backups")" >&2; exit 1; }
rm -rf "$app"
echo 'PASS checked adoption, refusals, recovery and removal through real Homebrew'
