#!/usr/bin/env bash
# Real plutil, permissions, Homebrew receipts and CLI reads on disposable Macs.
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
logs="$PWD/build/macos-native"
mkdir -p "$logs"
exec > >(tee "$logs/lifecycle.log") 2>&1
trap 'echo "macos-inventory FAILED at line $LINENO: $BASH_COMMAND" >&2' ERR
[[ -x $pkd ]]
command -v jq
brew --version
sw_vers

work=$(mktemp -d "$RUNNER_TEMP/pkgdeck-native.XXXXXX")
system_root='/Applications/PkgDeck CI Inventory'
user_root="$HOME/Applications/PkgDeck CI Inventory"
owned="$HOME/Applications/PkgDeck CI Owned.app"
tap=pkgdeck-ci/inventory
cask="$tap/pkgdeck-ci-owned"
for path in "$system_root" "$user_root" "$owned"; do
    [[ ! -e $path && ! -L $path ]] || { echo "Fixture already exists: $path"; exit 1; }
done
if brew tap | grep -Fxq "$tap"; then
    echo "Fixture tap already exists: $tap" >&2
    exit 1
fi
cleanup() {
    set +e
    chmod 755 "$system_root/Restricted" 2>/dev/null
    brew uninstall --cask "$cask"
    brew untap "$tap"
    rm -rf "$system_root" "$user_root" "$owned" "$work"
}
trap cleanup EXIT
mkdir -p "$system_root" "$user_root"
bundle() {
    local path=$1 identifier=$2
    mkdir -p "$path/Contents/MacOS"
    cat > "$path/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleIdentifier</key><string>$identifier</string>
<key>CFBundleName</key><string>PkgDeck CI Fixture</string>
<key>CFBundleExecutable</key><string>fixture</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>CFBundleShortVersionString</key><string>1.2.3</string>
<key>CFBundleVersion</key><string>456</string>
</dict></plist>
EOF
    printf '#!/bin/sh\ntouch "%s/launched"\n' "$work" > "$path/Contents/MacOS/fixture"
    chmod 755 "$path/Contents/MacOS/fixture"
}
bundle "$system_root/XML.app" com.microsoft.VSCode
bundle "$user_root/Binary.app" org.mozilla.firefox
/usr/bin/plutil -convert binary1 -- "$user_root/Binary.app/Contents/Info.plist"
bundle "$system_root/Store.app" md.obsidian
mkdir -p "$system_root/Store.app/Contents/_MASReceipt"
touch "$system_root/Store.app/Contents/_MASReceipt/receipt"
bundle "$system_root/Broken.app" md.obsidian
printf 'invalid plist' > "$system_root/Broken.app/Contents/Info.plist"
mkdir -p "$system_root/Empty.app/Contents"
bundle "$system_root/Utilities/Nested.app" md.obsidian
bundle "$system_root/.hidden/Invisible.app" md.obsidian
bundle "$system_root/XML.app/Contents/Helper.app" md.obsidian
ln -s "$system_root/XML.app" "$system_root/Z-alias.app"
ln -s "$system_root" "$system_root/Directory-loop"

# Install a local cask through real Homebrew, then create a second unmanaged copy.
bundle "$work/PkgDeck CI Owned.app" md.obsidian
ditto -c -k --keepParent "$work/PkgDeck CI Owned.app" "$work/fixture.zip"
digest=$(shasum -a 256 "$work/fixture.zip" | awk '{print $1}')
brew tap-new --no-git "$tap"
recipe="$(brew --repository "$tap")/Casks/pkgdeck-ci-owned.rb"
mkdir -p "$(dirname "$recipe")"
cat > "$recipe" <<EOF
cask "pkgdeck-ci-owned" do
  version "1.2.3"
  sha256 "$digest"
  url "file://$work/fixture.zip"
  name "PkgDeck CI Owned"
  desc "Disposable native inventory test fixture"
  homepage "https://github.com/astrovm/PkgDeck"
  app "PkgDeck CI Owned.app"
end
EOF
brew install --cask --appdir="$HOME/Applications" "$cask"
ditto "$owned" "$system_root/Second Copy.app"
brew info --json=v2 --cask --installed > "$logs/brew-installed.json"

"$pkd" --json --from macos-apps list > "$logs/inventory.json"
jq -e --arg root "$system_root/" --arg user "$user_root/" --arg owned "$owned" '
  .data | .failures == [] and
  ([.packages[] | select(.id.name | startswith($root))] | length == 6) and
  ([.packages[] | select(.id.name | startswith($user))] | length == 1) and
  any(.packages[]; .id.name == ($root + "XML.app") and .id.scope == "system" and .installed_version == "1.2.3" and (.summary | contains("visual-studio-code (candidate)"))) and
  any(.packages[]; .id.name == ($user + "Binary.app") and (.id.scope | type) == "object" and .installed_version == "1.2.3" and (.summary | contains("firefox (candidate)"))) and
  any(.packages[]; .id.name == ($root + "Store.app") and (.summary | contains("candidate") | not)) and
  any(.packages[]; .id.name == ($root + "Broken.app") and .installed_version == "unknown") and
  any(.packages[]; .id.name == ($root + "Empty.app") and .installed_version == "unknown") and
  any(.packages[]; .id.name == $owned and (.summary | contains("Managed by Homebrew")) and (.summary | contains("candidate") | not)) and
  any(.packages[]; .id.name == ($root + "Second Copy.app") and (.summary | contains("Managed by Homebrew") | not) and (.summary | contains("obsidian (candidate)")))
' "$logs/inventory.json"
"$pkd" --json --from macos-apps info "$user_root/Binary.app" > "$logs/details.json"
jq -e '.data | .package.installed_version == "1.2.3" and (.description | contains("Bundle build: 456"))' "$logs/details.json"
"$pkd" --json --from macos-apps search 'Second Copy' > "$logs/search.json"
jq -e --arg path "$system_root/Second Copy.app" '.data | [.packages[].id.name] == [$path]' "$logs/search.json"

# Exercise a real OS permission failure; readable siblings must survive.
bundle "$system_root/Restricted/Invisible.app" md.obsidian
chmod 000 "$system_root/Restricted"
code=0
"$pkd" --json --from macos-apps list > "$logs/partial.json" || code=$?
[[ $code == 8 ]]
jq -e --arg root "$system_root/" '
  .data | any(.failures[]; .backend == "macos-apps" and (.error.InvalidResponse.reason | contains($root + "Restricted"))) and
  ([.packages[] | select(.id.name | startswith($root))] | length == 6)
' "$logs/partial.json"

# Unsupported writes must never request approval or touch installed bundles.
for verb in install remove upgrade; do
    for approval in no yes; do
        args=(--json --from macos-apps "$verb" "$owned")
        if [[ $approval == yes ]]; then args+=(--yes); fi
        code=0
        "$pkd" "${args[@]}" > "$logs/$verb-$approval.json" || code=$?
        [[ $code == 1 ]]
        jq -e '.data | .error.Unsupported.backend == "macos-apps" and (has("operations") | not)' "$logs/$verb-$approval.json"
    done
done
[[ -d $owned && ! -e $work/launched ]]
chmod 755 "$system_root/Restricted"
brew uninstall --cask "$cask"
[[ ! -e $owned && -d $system_root/Second\ Copy.app ]]
"$pkd" --json --from macos-apps info "$system_root/Second Copy.app" > "$logs/after-uninstall.json"
jq -e '.data | (.package.summary | contains("Managed by Homebrew") | not) and (.package.summary | contains("obsidian (candidate)"))' "$logs/after-uninstall.json"
echo 'PASS native bundle discovery, details, scopes, real cask ownership, permissions and read-only CLI'
