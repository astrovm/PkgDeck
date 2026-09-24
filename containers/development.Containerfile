FROM docker.io/library/rust:1.98.1-slim@sha256:f47a8de237dcbb0b0ce1099901e60a89728e3d51f24e664b40e947171538ade7 AS rust
FROM docker.io/library/ubuntu:26.04@sha256:da6fc2be547864451aa253836dd926da33623312df4a9a243e35dc877c378a78
ENV DEBIAN_FRONTEND=noninteractive
RUN apt-get update && apt-get install -y --no-install-recommends \
    build-essential ninja-build lld pkg-config curl ca-certificates git libapt-pkg-dev jq libarchive-tools cmake \
    qt6-base-dev qt6-declarative-dev qt6-declarative-dev-tools qt6-tools-dev qt6-shadertools-dev qt6-wayland libkirigami-dev extra-cmake-modules \
    libgl1-mesa-dev libegl1-mesa-dev libxkbcommon-dev libvulkan-dev \
    libxcb-cursor0 libxcb-icccm4 libxcb-keysyms1 libxcb-shape0 libxcb-xkb1 \
    libxkbcommon-x11-0 libxcb-image0 libxcb-render-util0 libxcb-randr0 libxcb-sync1 \
    libxcb-xfixes0 libxcb-glx0 libdbus-1-3 libfontconfig1 libfreetype6 libopengl0 \
    libglib2.0-0t64 libpulse0 libgssapi-krb5-2 libnss3 libasound2t64 libxcomposite1 libxdamage1 \
    libxtst6 libwayland-cursor0 libwayland-egl1 \
    && rm -rf /var/lib/apt/lists/*
COPY --from=rust /usr/local/rustup /opt/rustup
COPY --from=rust /usr/local/cargo/bin/ /usr/local/bin/
ENV RUSTUP_HOME=/opt/rustup PATH=/usr/local/bin:/usr/local/sbin:/usr/sbin:/usr/bin:/sbin:/bin
WORKDIR /opt/bootstrap
COPY rust-toolchain.toml ./
COPY scripts/setup-dev.sh scripts/dev-env.sh ./scripts/
RUN cargo install --root /usr/local cargo-llvm-cov --version 0.9.1 --locked \
    && scripts/setup-dev.sh \
    && dpkg-query -W > /opt/pkgdeck-os-packages.txt
RUN apt-get update && apt-get install -y --no-install-recommends xvfb xdotool fonts-dejavu-core qt6-svg-plugins qt6-image-formats-plugins && rm -rf /var/lib/apt/lists/*
WORKDIR /workspace
ENV CARGO_HOME=/cache/cargo HOME=/tmp/pkgdeck-home
CMD ["scripts/verify.sh", "full"]
