FROM docker.io/library/rust:1.98.1-slim@sha256:ce84a5edd80c5f91e05c5533b1e53eb1da54028f33734dc06aa6b49fa190462d AS rust
FROM docker.io/library/ubuntu:26.04@sha256:513c074113a871b51a8d16ab445c88779d6452d937a164fb5cc479f32668a41d
ENV DEBIAN_FRONTEND=noninteractive
RUN apt-get update && apt-get install -y --no-install-recommends \
    build-essential ninja-build lld pkg-config curl ca-certificates git python3 python3-venv \
    libgl1-mesa-dev libegl1-mesa-dev libxkbcommon-dev libvulkan-dev \
    libxcb-cursor0 libxcb-icccm4 libxcb-keysyms1 libxcb-shape0 libxcb-xkb1 \
    libxkbcommon-x11-0 libxcb-image0 libxcb-render-util0 libxcb-randr0 libxcb-sync1 \
    libxcb-xfixes0 libxcb-glx0 libdbus-1-3 libfontconfig1 libfreetype6 libopengl0 \
    libglib2.0-0t64 libpulse0 libgssapi-krb5-2 libnss3 libasound2t64 libxcomposite1 libxdamage1 \
    libxtst6 libwayland-cursor0 libwayland-egl1 \
    && rm -rf /var/lib/apt/lists/*
COPY --from=rust /usr/local/rustup /opt/rustup
COPY --from=rust /usr/local/cargo/bin/ /usr/local/bin/
ENV RUSTUP_HOME=/opt/rustup PKGDECK_CACHE_DIR=/opt/pkgdeck-sdk
WORKDIR /opt/bootstrap
COPY rust-toolchain.toml ./
COPY scripts/setup-dev.sh scripts/dev-env.sh scripts/build-kirigami.sh scripts/sdk.sha256 ./scripts/
RUN scripts/setup-dev.sh && rm -rf /opt/pkgdeck-sdk/sdk-build \
    && dpkg-query -W > /opt/pkgdeck-os-packages.txt
RUN apt-get update && apt-get install -y --no-install-recommends xvfb xdotool fonts-dejavu-core && rm -rf /var/lib/apt/lists/*
WORKDIR /workspace
ENV CARGO_HOME=/cache/cargo HOME=/tmp/pkgdeck-home
CMD ["scripts/verify.sh", "full"]
