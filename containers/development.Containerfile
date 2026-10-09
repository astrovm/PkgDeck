FROM docker.io/library/rust:1.98.1-slim@sha256:4cd829461bd5c4d511c32e269da9cb8929223b666519d8004e35fc8d1d771ab7 AS rust
FROM docker.io/library/ubuntu:26.04@sha256:da6fc2be547864451aa253836dd926da33623312df4a9a243e35dc877c378a78
ENV DEBIAN_FRONTEND=noninteractive
RUN apt-get update && apt-get install -y --no-install-recommends \
    build-essential ninja-build lld pkg-config curl ca-certificates git libapt-pkg-dev jq libarchive-tools cmake \
    libgl1 libegl1 libgl1-mesa-dri libxkbcommon0 libxkbcommon-x11-0 \
    libx11-6 libx11-xcb1 libxcursor1 libxrandr2 libxi6 libwayland-client0 \
    libwayland-cursor0 libwayland-egl1 libdbus-1-3 \
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
RUN apt-get update && apt-get install -y --no-install-recommends xvfb xdotool fonts-dejavu-core && rm -rf /var/lib/apt/lists/*
WORKDIR /workspace
ENV CARGO_HOME=/cache/cargo HOME=/tmp/pkgdeck-home
CMD ["scripts/verify.sh", "full"]
