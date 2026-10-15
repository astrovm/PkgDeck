#!/usr/bin/env bash
# Create a persistent self-signed identity once, not once per release. Keep
# its encrypted archive private and back it up outside the repository.
set -euo pipefail
destination=${1:?Usage: create-macos-signing.sh /private/backup/folder}
[[ ! -e $destination ]] || { echo 'Choose a new folder; an existing identity must not be overwritten.' >&2; exit 1; }
umask 077
work=$(mktemp -d)
# Everything is made in a scratch folder and moved in at the end, so a
# failed run leaves no half-made identity behind to block the next one.
cleanup() {
    for file in key.pem cert.pem req.log identity/password.txt identity/signing.p12 identity/certificate.pem; do
        unlink "$work/$file" 2>/dev/null || true
    done
    rmdir "$work/identity" 2>/dev/null || true
    rmdir "$work"
}
trap cleanup EXIT
mkdir "$work/identity"
openssl rand -hex 32 > "$work/identity/password.txt"
# Its progress dots are noise; its errors are shown if it fails.
openssl req -x509 -newkey rsa:3072 -nodes -keyout "$work/key.pem" -out "$work/cert.pem" \
    -days 3650 -subj '/CN=PkgDeck Release Signing' \
    -addext 'basicConstraints=critical,CA:FALSE' \
    -addext 'keyUsage=critical,digitalSignature' \
    -addext 'extendedKeyUsage=critical,codeSigning' 2>"$work/req.log" ||
    { cat "$work/req.log" >&2; exit 1; }
# macOS security import expects the older PKCS#12 encryption algorithms,
# which OpenSSL 3 only writes when asked.
pkcs12_args=(-export)
if [[ $(openssl version) == 'OpenSSL 3'* ]]; then pkcs12_args+=(-legacy); fi
openssl pkcs12 "${pkcs12_args[@]}" -inkey "$work/key.pem" -in "$work/cert.pem" \
    -out "$work/identity/signing.p12" -passout "file:$work/identity/password.txt"
openssl x509 -in "$work/cert.pem" -out "$work/identity/certificate.pem"
mkdir -p "$(dirname "$destination")"
mv "$work/identity" "$destination"
echo "Created the signing archive in $destination. Keep this folder private and reuse it for every release."
