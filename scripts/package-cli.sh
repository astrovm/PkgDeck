#!/usr/bin/env bash
# Build static Linux pkd and host runner binaries for the Homebrew formula,
# with the AppImage updater every Linux build ships.
# musl makes one archive per architecture run on any distribution.
set -euo pipefail
[[ $(uname -s) == Linux ]] || { echo 'package-cli.sh requires Linux' >&2; exit 1; }
cd "$(dirname "${BASH_SOURCE[0]}")/.."
version=${PKGDECK_VERSION:-$(sed -n 's/^version = "\([^"]*\)"/\1/p' Cargo.toml | head -n1)}
[[ $version =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || { echo "Invalid PkgDeck version: $version" >&2; exit 1; }
arch=$(uname -m)
target=$arch-unknown-linux-musl
rustup target add "$target"
cargo build --locked --release --target "$target" -p pkd -p pkgdeck-core --bin pkd --bin pkgdeck-host-runner
built=${CARGO_TARGET_DIR:-target}/$target/release
name=PkgDeck-v$version-linux-$arch-cli
stage=build/$name
rm -rf "$stage"
mkdir -p "$stage/bin" "$stage/libexec" build/artifacts
cp "$built/pkd" "$stage/bin/"
cp "$built/pkgdeck-host-runner" "$stage/libexec/"
scripts/appimage-updater.sh "$stage"
scripts/bundle-cli-apt.sh "$stage"
cp LICENSE "$stage/"
for binary in "$stage/bin/pkd" "$stage/libexec/pkgdeck-host-runner"; do
    file "$binary" | grep -Eq 'statically linked|static-pie linked' || { file "$binary"; echo "$binary is not static" >&2; exit 1; }
done
tar -C build -czf "build/artifacts/$name.tar.gz" "$name"
echo "Built build/artifacts/$name.tar.gz"
