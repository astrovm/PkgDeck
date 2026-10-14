#!/usr/bin/env bash
# Real macOS signing, with two disposable publisher identities. No trust or
# privacy settings are changed, and no installed app is touched.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/../.."
[[ $(uname -s) == Darwin ]] || { echo 'Requires macOS' >&2; exit 1; }
work=$(mktemp -d)
cleanup() {
    for keychain in "$work"/pkgdeck-signing.keychain-db "$work"/other/pkgdeck-signing.keychain-db; do
        [[ ! -f $keychain ]] || security delete-keychain "$keychain"
    done
    # The tree was created by this test; find does not follow symlinks.
    find "$work" -depth -delete
}
trap cleanup EXIT
fail() { echo "$*" >&2; exit 1; }
expect_failure() {
    if "$@" >"$work/failure.log" 2>&1; then fail "Unexpected success: $*"; fi
}
scripts/create-macos-signing.sh "$work/publisher"
expect_failure scripts/create-macos-signing.sh "$work/publisher"
export MACOS_SIGNING_P12 MACOS_SIGNING_PASSWORD
MACOS_SIGNING_P12=$(base64 < "$work/publisher/signing.p12")
MACOS_SIGNING_PASSWORD=$(cat "$work/publisher/password.txt")
RUNNER_TEMP="$work" GITHUB_ENV="$work/env" scripts/import-macos-signing.sh
# shellcheck disable=SC1091 # Written by the import script above.
source "$work/env"
export PKGDECK_SIGNING_IDENTITY PKGDECK_SIGNING_KEYCHAIN
export PKGDECK_REQUIRE_SIGNING=1
app=$work/PkgDeck.app
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cat > "$app/Contents/Info.plist" <<'EOF'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleIdentifier</key><string>io.github.astrovm.PkgDeck</string>
<key>CFBundleExecutable</key><string>pkgdeck</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>CFBundleVersion</key><string>1</string>
</dict></plist>
EOF
compile() {
    printf 'int main(void) { return %s; }\n' "$1" | clang -x c - -o "$app/Contents/MacOS/pkgdeck"
    cp "$app/Contents/MacOS/pkgdeck" "$app/Contents/MacOS/pkd"
}
compile 1
scripts/sign-macos.sh "$app"
codesign --display -r- "$app" 2>/dev/null | sed 's/^designated => //' > "$work/app.req"
codesign --display -r- "$app/Contents/MacOS/pkd" 2>/dev/null | sed 's/^designated => //' > "$work/tool.req"
first=$(codesign --display --verbose=4 "$app" 2>&1 | sed -n 's/^CDHash=//p')
compile 2
plutil -replace CFBundleVersion -string 2 "$app/Contents/Info.plist"
scripts/sign-macos.sh "$app"
second=$(codesign --display --verbose=4 "$app" 2>&1 | sed -n 's/^CDHash=//p')
[[ -n $first && -n $second && $first != "$second" ]] || fail 'The update did not change the code hash'
codesign --verify --strict -R "$work/app.req" "$app"
codesign --verify --strict -R "$work/tool.req" "$app/Contents/MacOS/pkd"
codesign --display -r- "$app" 2>/dev/null | sed 's/^designated => //' > "$work/updated.req"
cmp "$work/app.req" "$work/updated.req"
expect_failure env PKGDECK_SIGNING_IDENTITY=- scripts/sign-macos.sh "$app"
grep -q 'certificate missing' "$work/failure.log"
expect_failure env PKGDECK_SIGNING_IDENTITY=does-not-exist scripts/sign-macos.sh "$app"
printf 'tampered' > "$app/Contents/Resources/extra"
expect_failure codesign --verify --deep --strict "$app"
scripts/sign-macos.sh "$app"
codesign --verify --strict -R "$work/app.req" "$app"
mkdir "$work/other"
scripts/create-macos-signing.sh "$work/other/publisher"
MACOS_SIGNING_P12=$(base64 < "$work/other/publisher/signing.p12")
MACOS_SIGNING_PASSWORD=$(cat "$work/other/publisher/password.txt")
RUNNER_TEMP="$work/other" GITHUB_ENV="$work/other/env" scripts/import-macos-signing.sh
# shellcheck disable=SC1091 # Written by the import script above.
source "$work/other/env"
scripts/sign-macos.sh "$app"
expect_failure codesign --verify --strict -R "$work/app.req" "$app"
expect_failure codesign --verify --strict -R "$work/tool.req" "$app/Contents/MacOS/pkd"
# Ad-hoc development builds still work, but cannot impersonate the release.
PKGDECK_REQUIRE_SIGNING=0 PKGDECK_SIGNING_IDENTITY=- scripts/sign-macos.sh "$app"
expect_failure codesign --verify --strict -R "$work/app.req" "$app"
echo 'PASS updated app and CLI retain their identity; foreign, missing, invalid and tampered signatures are rejected'
