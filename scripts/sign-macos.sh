#!/usr/bin/env bash
# Sign nested tools before their enclosing bundle. Reuse the certificate on
# every release so macOS can match the app's designated requirement.
set -euo pipefail
app=${1:?Usage: sign-macos.sh PkgDeck.app}
identity=${PKGDECK_SIGNING_IDENTITY:--}
if [[ $identity == - && ${PKGDECK_REQUIRE_SIGNING:-0} == 1 ]]; then
    echo 'Release signing certificate missing. Configure MACOS_SIGNING_P12 and MACOS_SIGNING_PASSWORD.' >&2
    exit 1
fi
args=(--force --sign "$identity" --timestamp=none)
if [[ -n ${PKGDECK_SIGNING_KEYCHAIN:-} ]]; then
    args+=(--keychain "$PKGDECK_SIGNING_KEYCHAIN")
fi
codesign "${args[@]}" --identifier io.github.astrovm.PkgDeck.pkd "$app/Contents/MacOS/pkd"
codesign "${args[@]}" --identifier io.github.astrovm.PkgDeck "$app"
codesign --verify --deep --strict "$app"
if [[ $identity != - ]]; then
    requirement=$(codesign --display -r- "$app" 2>/dev/null)
    # A typo, expired identity or unsuitable certificate must never silently
    # turn a release back into an ad-hoc build.
    [[ $requirement == *certificate* && $requirement != *cdhash* ]] || {
        echo 'Signing did not produce a certificate-based app identity.' >&2
        exit 1
    }
else
    echo 'Development bundle is ad-hoc signed; macOS approvals may not survive rebuilding.' >&2
fi
