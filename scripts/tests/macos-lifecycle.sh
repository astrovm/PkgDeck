#!/usr/bin/env bash
# Requires a graphical macOS session and Accessibility access for the terminal.
set -euo pipefail
[[ $(uname -s) == Darwin ]] || { echo 'This check requires macOS' >&2; exit 1; }
source_app=${1:?Usage: macos-lifecycle.sh /path/PkgDeck.app-or-binary}
test_root=$(mktemp -d)
trap 'rm -rf "$test_root"' EXIT
app="$test_root/PkgDeck Lifecycle Test.app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources" "$test_root/config/pkgdeck"
if [[ -d $source_app ]]; then
    cp "$source_app/Contents/MacOS/pkgdeck" "$app/Contents/MacOS/pkgdeck"
    cp "$source_app/Contents/Resources/PkgDeck.icns" "$app/Contents/Resources/PkgDeck.icns"
else
    cp "$source_app" "$app/Contents/MacOS/pkgdeck"
fi
cat > "$app/Contents/Info.plist" <<'XML'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>CFBundleIdentifier</key><string>io.github.astrovm.PkgDeck.LifecycleTest</string>
  <key>CFBundleExecutable</key><string>pkgdeck</string>
  <key>CFBundleName</key><string>PkgDeck Lifecycle Test</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleIconFile</key><string>PkgDeck</string>
</dict></plist>
XML
cat > "$test_root/config/pkgdeck/window.json" <<'JSON'
{"setupAsked":true,"sourceList":"fixture-unavailable","autoUpdate":false,"autoClean":false,"backgroundMode":true,"checkInterval":0}
JSON
codesign --force --sign - "$app"
clang -fobjc-arc -fblocks -Wall -Wextra -Werror -framework AppKit -framework ApplicationServices \
    "$(dirname "${BASH_SOURCE[0]}")/macos-lifecycle.m" -o "$test_root/lifecycle"
"$test_root/lifecycle" "$app" "$test_root"
