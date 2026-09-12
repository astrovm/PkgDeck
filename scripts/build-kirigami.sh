#!/usr/bin/env bash
set -euo pipefail
# Qt 6.11.2 must already be on PATH/CMAKE_PREFIX_PATH.
: "${PKGDECK_SDK_PREFIX:?Set PKGDECK_SDK_PREFIX to the install directory}"
work="${PKGDECK_BUILD_ROOT:-$PWD/build/sdk}"
mkdir -p "$work"
for project in extra-cmake-modules kirigami; do
    archive="$work/$project-6.30.0.tar.xz"
    curl --fail --location --retry 3 "https://download.kde.org/stable/frameworks/6.30/$project-6.30.0.tar.xz" -o "$archive"
    curl --fail --location --retry 3 "https://download.kde.org/stable/frameworks/6.30/$project-6.30.0.tar.xz.sha256" -o "$archive.sha256"
    (cd "$work" && sha256sum --check "$project-6.30.0.tar.xz.sha256")
    tar -xf "$archive" -C "$work"
    cmake -S "$work/$project-6.30.0" -B "$work/$project-build" -G Ninja \
        -DCMAKE_BUILD_TYPE=Release -DCMAKE_INSTALL_PREFIX="$PKGDECK_SDK_PREFIX" \
        -DCMAKE_INSTALL_LIBDIR=lib -DKDE_INSTALL_LIBDIR=lib \
        -DKDE_INSTALL_QMLDIR=qml -DBUILD_TESTING=OFF -DBUILD_EXAMPLES=OFF
    cmake --build "$work/$project-build" --parallel 2
    cmake --install "$work/$project-build"
    export CMAKE_PREFIX_PATH="$PKGDECK_SDK_PREFIX:${CMAKE_PREFIX_PATH:-}"
done
