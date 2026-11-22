# Builds the Rust programs for the AppImage and Snap. They use the host's glibc,
# so they only start where it is at least as new as the build system's. Ubuntu
# 22.04 is the oldest Ubuntu LTS in standard support, and its glibc 2.35 sets
# the minimum. The APT headers are left out on purpose: the packages ship the APT
# reader from scripts/bundle-apt.sh, built with a newer APT.
FROM docker.io/library/rust:1.100.0-slim@sha256:6f14b2b4b278ec0d50aa1d69541c304ce0f95e0f6268487aa9924af1c2e9aebe AS rust
FROM docker.io/library/ubuntu:22.04@sha256:f64b2ffc2f10ab8f8505aa56b2664bd846cbb9df87c7e2532e30f7667021c0e6
ENV DEBIAN_FRONTEND=noninteractive
COPY scripts/ci/apt-mirrors.sh /opt/pkgdeck/apt-mirrors.sh
RUN bash /opt/pkgdeck/apt-mirrors.sh
RUN apt-get update && apt-get install -y --no-install-recommends \
    build-essential pkg-config ca-certificates \
    libxkbcommon0 libxkbcommon-x11-0 libxcursor1 libxi6 \
    && rm -rf /var/lib/apt/lists/*
COPY --from=rust /usr/local/rustup /opt/rustup
COPY --from=rust /usr/local/cargo/bin/ /usr/local/bin/
ENV RUSTUP_HOME=/opt/rustup PATH=/usr/local/bin:/usr/local/sbin:/usr/sbin:/usr/bin:/sbin:/bin
WORKDIR /opt/bootstrap
COPY rust-toolchain.toml ./
# Running cargo here installs the toolchain rust-toolchain.toml names.
RUN cargo --version && rustc --version
WORKDIR /workspace
ENV CARGO_HOME=/cache/cargo HOME=/tmp/pkgdeck-home
CMD ["cargo", "build", "--workspace", "--release", "--locked"]
