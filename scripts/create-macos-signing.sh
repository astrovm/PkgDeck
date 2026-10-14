#!/usr/bin/env bash
# Create a persistent self-signed identity once, not once per release. Keep
# its encrypted archive private and back it up outside the repository.
set -euo pipefail
destination=${1:?Usage: create-macos-signing.sh /private/backup/folder}
[[ ! -e $destination ]] || { echo 'Choose a new folder; an existing identity must not be overwritten.' >&2; exit 1; }
umask 077
mkdir -p "$destination"
work=$(mktemp -d)
cleanup() {
    unlink "$work/key.pem" 2>/dev/null || true
    unlink "$work/cert.pem" 2>/dev/null || true
    rmdir "$work"
}
trap cleanup EXIT
openssl rand -hex 32 > "$destination/password.txt"
openssl req -x509 -newkey rsa:3072 -nodes -keyout "$work/key.pem" -out "$work/cert.pem" \
    -days 3650 -subj '/CN=PkgDeck Release Signing' \
    -addext 'basicConstraints=critical,CA:FALSE' \
    -addext 'keyUsage=critical,digitalSignature' \
    -addext 'extendedKeyUsage=critical,codeSigning' 2>/dev/null
# macOS security import expects the older PKCS#12 encryption algorithms.
pkcs12_args=(-export)
if [[ $(openssl version) == OpenSSL* ]]; then pkcs12_args+=(-legacy); fi
openssl pkcs12 "${pkcs12_args[@]}" -inkey "$work/key.pem" -in "$work/cert.pem" \
    -out "$destination/signing.p12" -passout "file:$destination/password.txt"
openssl x509 -in "$work/cert.pem" -out "$destination/certificate.pem"
echo "Created the signing archive in $destination. Keep this folder private and reuse it for every release."
