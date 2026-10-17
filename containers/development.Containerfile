FROM docker.io/library/rust:1.99.1-slim@sha256:4df2ae8cfdfebdfc13a65da7f253b73d68983dc0339f2719bc17a279c913b13d AS rust
FROM docker.io/library/ubuntu:26.04@sha256:2e27ff302056b2b96df4a524ce48e72c595abfe3fd7a3d923adf8e782e16896f
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
