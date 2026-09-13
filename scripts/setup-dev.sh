#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."
source scripts/dev-env.sh
for tool in python3 rustup cargo curl tar ninja pkg-config c++ ld.lld flock; do
    command -v "$tool" >/dev/null || { echo "Missing $tool; install the development prerequisites in docs/development.md." >&2; exit 1; }
done
python3 -c 'import sys; assert sys.version_info[:2] == (3, 14), "SDK setup requires Python 3.14 (Qt archive support)"'
mkdir -p "$PKGDECK_CACHE_DIR"
# Serialize setup in a shared cache; failed downloads never create completion markers.
exec 9>"$PKGDECK_CACHE_DIR/setup.lock"
flock 9
rustup show
if [[ ! -x "$PKGDECK_CACHE_DIR/python/bin/python" ]]; then
    python3 -m venv "$PKGDECK_CACHE_DIR/python"
fi
"$PKGDECK_CACHE_DIR/python/bin/python" -m pip install --disable-pip-version-check aqtinstall==3.3.0 cmake==4.4.3
if [[ ! -f "$QT_ROOT_DIR/.pkgdeck-complete" ]] || [[ ! -x "$QT_ROOT_DIR/bin/qtpaths" ]] || [[ $("$QT_ROOT_DIR/bin/qtpaths" --qt-version) != 6.11.2 ]]; then
    case $(uname -m) in
        x86_64) qt_host=linux; qt_arch=linux_gcc_64 ;;
        aarch64) qt_host=linux_arm64; qt_arch=linux_gcc_arm64 ;;
    esac
    # aqt verifies the upstream Qt archive checksums during installation.
    aqt install-qt "$qt_host" desktop 6.11.2 "$qt_arch" -O "$PKGDECK_CACHE_DIR/Qt" -m qtshadertools qtimageformats
    touch "$QT_ROOT_DIR/.pkgdeck-complete"
fi
sdk_key=$(cat scripts/build-kirigami.sh scripts/sdk.sha256 | sha256sum | cut -d ' ' -f 1)
if [[ ! -f "$PKGDECK_SDK_PREFIX/.pkgdeck-sdk" ]] || [[ $(cat "$PKGDECK_SDK_PREFIX/.pkgdeck-sdk") != "$sdk_key" ]]; then
    PKGDECK_BUILD_ROOT="$PKGDECK_CACHE_DIR/sdk-build" scripts/build-kirigami.sh
    printf '%s\n' "$sdk_key" > "$PKGDECK_SDK_PREFIX/.pkgdeck-sdk"
fi
if [[ ! -x "$PKGDECK_CACHE_DIR/tools/bin/cargo-llvm-cov" ]] || [[ $("$PKGDECK_CACHE_DIR/tools/bin/cargo-llvm-cov" llvm-cov --version) != 'cargo-llvm-cov 0.9.1' ]]; then
    cargo install cargo-llvm-cov --version 0.9.1 --locked --root "$PKGDECK_CACHE_DIR/tools"
fi
printf 'SDK ready at %s. Run scripts/verify.sh full.\n' "$PKGDECK_CACHE_DIR"
