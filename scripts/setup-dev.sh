#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."
source scripts/dev-env.sh
for tool in bsdtar rustup cargo curl tar ninja pkg-config c++ ld.lld flock; do
    command -v "$tool" >/dev/null || { echo "Missing $tool; install the development prerequisites in docs/development.md." >&2; exit 1; }
done
mkdir -p "$PKGDECK_CACHE_DIR"
# Serialize setup in a shared cache; failed downloads never create completion markers.
exec 9>"$PKGDECK_CACHE_DIR/setup.lock"
flock 9
rustup show
scripts/install-sdk.sh
sdk_key=$(cat scripts/build-kirigami.sh scripts/sdk.sha256 | sha256sum | cut -d ' ' -f 1)
if [[ ! -f "$PKGDECK_SDK_PREFIX/.pkgdeck-sdk" ]] || [[ $(cat "$PKGDECK_SDK_PREFIX/.pkgdeck-sdk") != "$sdk_key" ]]; then
    PKGDECK_BUILD_ROOT="$PKGDECK_CACHE_DIR/sdk-build" scripts/build-kirigami.sh
    printf '%s\n' "$sdk_key" > "$PKGDECK_SDK_PREFIX/.pkgdeck-sdk"
fi
if [[ ! -x "$PKGDECK_CACHE_DIR/tools/bin/cargo-llvm-cov" ]] || [[ $("$PKGDECK_CACHE_DIR/tools/bin/cargo-llvm-cov" llvm-cov --version) != 'cargo-llvm-cov 0.9.1' ]]; then
    cargo install cargo-llvm-cov --version 0.9.1 --locked --root "$PKGDECK_CACHE_DIR/tools"
fi
printf 'SDK ready at %s. Run scripts/verify.sh full.\n' "$PKGDECK_CACHE_DIR"
