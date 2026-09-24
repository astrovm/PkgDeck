#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/../.."

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
mkdir -p "$work/scripts" "$work/bin" "$work/build/AppDir" "$work/assets" "$work/packaging/snap" "$work/packaging/flatpak"
cp scripts/package.sh "$work/scripts/package.sh"
cp scripts/flatpak-release-manifest.sh "$work/scripts/flatpak-release-manifest.sh"
cp packaging/flatpak/io.github.astrovm.PkgDeck.yml "$work/packaging/flatpak/io.github.astrovm.PkgDeck.yml"
printf '[workspace.package]\nversion = "9.8.7"\n' > "$work/Cargo.toml"
printf '[Desktop Entry]\nIcon=synthetic\nExec=pkgdeck\n' > "$work/assets/io.github.astrovm.PkgDeck.desktop"
printf '<svg/>\n' > "$work/assets/io.github.astrovm.PkgDeck.svg"
cp packaging/snap/snap.yaml "$work/packaging/snap/snap.yaml"
cat > "$work/bin/uname" <<'EOF'
#!/bin/bash
[[ $1 == -m ]] || exit 2
printf '%s\n' "$PACKAGE_TEST_ARCH"
EOF
cat > "$work/bin/appimagetool" <<'EOF'
#!/bin/bash
[[ $# == 5 && $1 == --appimage-extract-and-run && $2 == -u ]] || exit 2
printf '%s\n' "$3" > "$TEST_CAPTURE"
: > "$5"
EOF
cat > "$work/bin/flatpak-builder" <<'EOF'
#!/bin/bash
exit 0
EOF
cat > "$work/bin/flatpak" <<'EOF'
#!/bin/bash
[[ $1 == build-bundle ]] || exit 2
: > "$3"
EOF
cat > "$work/bin/snap" <<'EOF'
#!/bin/bash
[[ $1 == pack ]] || exit 2
version=$(sed -n "s/^version: '\([^']*\)'/\1/p" "$2/meta/snap.yaml")
arch=$(sed -n 's/^architectures: \[\([^]]*\)\]/\1/p' "$2/meta/snap.yaml")
: > "$3/pkgdeck_${version}_${arch}.snap"
EOF
cat > "$work/bin/curl" <<'EOF'
#!/bin/bash
while [[ $# -gt 0 ]]; do
    if [[ $1 == -o ]]; then
        printf 'synthetic archive\n' > "$2"
        exit 0
    fi
    shift
done
exit 2
EOF
chmod +x "$work/scripts/package.sh" "$work/bin/"*

for arch in x86_64 aarch64; do
    case "$arch" in x86_64) snap_arch=amd64 ;; aarch64) snap_arch=arm64 ;; esac
    for format in appimage flatpak snap; do
        (cd "$work" && PATH="$work/bin:$PATH" PACKAGE_TEST_ARCH="$arch" \
            APPIMAGETOOL="$work/bin/appimagetool" TEST_CAPTURE="$work/update-info" \
            scripts/package.sh "$format")
    done
    name="PkgDeck-v9.8.7-$arch"
    test -f "$work/build/artifacts/$name.AppImage"
    test -f "$work/build/artifacts/$name.flatpak"
    test -f "$work/build/artifacts/$name.snap"
    grep -Fxq "gh-releases-zsync|astrovm|PkgDeck|latest|PkgDeck-v*-$arch.AppImage.zsync" "$work/update-info"
    grep -Fxq "version: '9.8.7'" "$work/build/snap/meta/snap.yaml"
    grep -Fxq "architectures: [$snap_arch]" "$work/build/snap/meta/snap.yaml"
done

(cd "$work" && PATH="$work/bin:$PATH" scripts/flatpak-release-manifest.sh v9.8.7 release.yml)
grep -Fq 'https://github.com/astrovm/PkgDeck/archive/refs/tags/v9.8.7.tar.gz' "$work/release.yml"
if grep -q 'cargo-sources' "$work/release.yml"; then
    echo 'Release manifest still references offline Cargo sources' >&2
    exit 1
fi
grep -Fq -- '--share=network' "$work/release.yml"
grep -Fq 'cargo build --release --locked' "$work/release.yml"
grep -Fq "$(printf 'synthetic archive\n' | sha256sum | cut -d' ' -f1)" "$work/release.yml"

if (cd "$work" && PKGDECK_VERSION=invalid scripts/package.sh appimage) > /dev/null 2>&1; then
    echo 'Invalid package versions must fail' >&2
    exit 1
fi
echo 'PASS versioned AppImage, Flatpak, and Snap names and update metadata'
