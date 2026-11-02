#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."
source scripts/dev-env.sh
for tool in rustup cargo pkg-config c++ ld.lld; do
    command -v "$tool" >/dev/null || { echo "Missing $tool; install the development prerequisites in docs/development.md." >&2; exit 1; }
done
rustup show
# Name the pinned toolchain: cargo install warns when the toolchain file picks it.
toolchain=$(sed -n 's/^channel = "\(.*\)"$/\1/p' rust-toolchain.toml)
command -v cargo-llvm-cov >/dev/null || cargo "+$toolchain" install cargo-llvm-cov --version 0.9.1 --locked
printf 'Development toolchain ready. Run scripts/verify.sh full.\n'
