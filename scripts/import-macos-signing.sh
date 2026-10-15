#!/usr/bin/env bash
# Import a persistent release identity into a temporary CI keychain. This is
# only called on trusted main/tag builds, never on pull request code.
set -euo pipefail
: "${MACOS_SIGNING_P12:?Configure the base64-encoded signing certificate and private key}"
: "${MACOS_SIGNING_PASSWORD:?Configure the signing archive password}"
: "${RUNNER_TEMP:?}"
: "${GITHUB_ENV:?}"
umask 077
keychain=$RUNNER_TEMP/pkgdeck-signing.keychain-db
archive=$(mktemp "$RUNNER_TEMP/pkgdeck-signing.XXXXXX")
trap 'unlink "$archive"' EXIT
password=$(openssl rand -hex 32)
printf '::add-mask::%s\n' "$password"
printf '%s' "$MACOS_SIGNING_P12" | base64 --decode > "$archive"
security create-keychain -p "$password" "$keychain"
security set-keychain-settings -lut 21600 "$keychain"
security unlock-keychain -p "$password" "$keychain"
security import "$archive" -f pkcs12 -k "$keychain" -P "$MACOS_SIGNING_PASSWORD" -T /usr/bin/codesign
security set-key-partition-list -S apple-tool:,apple:,codesign: -s -k "$password" "$keychain" >/dev/null
# Older macOS signing tools also need the keychain in the user search list,
# even with codesign --keychain. Preserve every existing keychain; deleting
# this temporary keychain at the end removes only its own search-list entry.
search=()
while IFS= read -r entry; do
    entry=${entry#*\"}
    entry=${entry%\"*}
    [[ $entry == "$keychain" ]] || search+=("$entry")
done < <(security list-keychains -d user)
security list-keychains -d user -s ${search[@]+"${search[@]}"} "$keychain"
identities=$(security find-identity -p codesigning "$keychain" |
    awk '/[0-9]+\) [0-9A-F]{40} / {print $2}' | sort -u)
[[ -n $identities && $(wc -w <<< "$identities") -eq 1 ]] || {
    echo 'The signing archive must contain exactly one code-signing identity.' >&2
    exit 1
}
printf 'PKGDECK_SIGNING_KEYCHAIN=%s\nPKGDECK_SIGNING_IDENTITY=%s\n' "$keychain" "$identities" >> "$GITHUB_ENV"
