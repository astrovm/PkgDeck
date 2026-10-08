#!/usr/bin/env bash
# Build a self-contained PkgDeck.app (GUI and pkd), then zip it for the
# Homebrew cask.
set -euo pipefail
[[ $(uname -s) == Darwin ]] || { echo 'bundle-macos.sh requires macOS' >&2; exit 1; }
cd "$(dirname "${BASH_SOURCE[0]}")/.."
version=${PKGDECK_VERSION:-$(sed -n 's/^version = "\([^"]*\)"/\1/p' Cargo.toml | head -n1)}
[[ $version =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || { echo "Invalid PkgDeck version: $version" >&2; exit 1; }
case "$(uname -m)" in
    arm64) arch=aarch64 ;;
    x86_64) arch=x86_64 ;;
    *) echo "Unsupported macOS architecture: $(uname -m)" >&2; exit 1 ;;
esac
work=$PWD/build/macos
app=$work/PkgDeck.app
contents=$app/Contents
target=${CARGO_TARGET_DIR:-target}/release
mkdir -p "$work" build/artifacts

cargo build --locked --release -p pkgdeck -p pkd

rm -rf "$app"
mkdir -p "$contents/MacOS" "$contents/Resources" "$contents/Resources/licenses"
cp "$target/pkgdeck" "$target/pkd" "$contents/MacOS/"
cp LICENSE "$contents/Resources/licenses/PkgDeck"
cp crates/pkgdeck/assets/fonts/Inter-LICENSE.txt "$contents/Resources/licenses/Inter"
# sudo's password prompt for Homebrew casks that need administrator access.
install -m 755 packaging/macos/pkgdeck-askpass "$contents/Resources/pkgdeck-askpass"
iconset=$work/PkgDeck.iconset
rm -rf "$iconset"
mkdir -p "$iconset"
for pair in 16:16x16 32:16x16@2x 32:32x32 64:32x32@2x 128:128x128 256:128x128@2x \
    256:256x256 512:256x256@2x 512:512x512 1024:512x512@2x; do
    rsvg-convert -w "${pair%%:*}" -h "${pair%%:*}" assets/io.github.astrovm.PkgDeck.svg \
        -o "$iconset/icon_${pair#*:}.png"
done
iconutil -c icns "$iconset" -o "$contents/Resources/PkgDeck.icns"

# Every Mach-O file must load only system libraries or bundled copies. One
# file(1) call finds them among the ~2000 files; running it per file is slow.
leaks=
minos=
while IFS= read -r -d '' file && IFS= read -r kind; do
    [[ $kind == *Mach-O* ]] || continue
    load=$(otool -l "$file")
    # Build-time rpaths would let a developer's Homebrew mask a missing copy.
    awk '/cmd LC_RPATH/ {getline; getline; print $2}' <<<"$load" | while IFS= read -r rpath; do
        [[ $rpath == @* ]] || install_name_tool -delete_rpath "$rpath" "$file"
    done
    minos+=$(awk '/minos/ {print $2}' <<<"$load")$'\n'
    # A copied library keeps its original install name as its ID; only the
    # libraries it loads matter.
    id=$(otool -D "$file" | sed -n 2p)
    if otool -L "$file" | tail -n +2 | awk '{print $1}' | grep -vxF "${id:-/}" |
        grep -Ev '^(/usr/lib/|/System/|@)'; then
        leaks+="$file"$'\n'
    fi
done < <(find "$app" -type f -exec file -0 {} +)
[[ -z $leaks ]] || { printf 'Unbundled library references in:\n%s' "$leaks" >&2; exit 1; }

# The newest minimum OS among bundled binaries is the app's minimum.
minimum=$(grep . <<<"$minos" | sort -t. -k1,1n -k2,2n | tail -n 1)
cat > "$contents/Info.plist" <<XML
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>CFBundleIdentifier</key><string>io.github.astrovm.PkgDeck</string>
  <key>CFBundleName</key><string>PkgDeck</string>
  <key>CFBundleDisplayName</key><string>PkgDeck</string>
  <key>CFBundleExecutable</key><string>pkgdeck</string>
  <key>CFBundleIconFile</key><string>PkgDeck</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>$version</string>
  <key>CFBundleVersion</key><string>$version</string>
  <key>LSMinimumSystemVersion</key><string>$minimum</string>
  <key>LSApplicationCategoryType</key><string>public.app-category.developer-tools</string>
  <key>NSHighResolutionCapable</key><true/>
</dict></plist>
XML

# Without a paid Developer ID the app is ad-hoc signed; Apple Silicon refuses
# to run unsigned code, and the cask clears the download quarantine.
codesign --force --deep --sign - "$app"
codesign --verify --deep --strict "$app"
zip=build/artifacts/PkgDeck-v$version-macos-$arch.zip
rm -f "$zip"
ditto -c -k --keepParent "$app" "$zip"
echo "Built $zip (macOS $minimum or later)"
