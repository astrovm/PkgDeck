#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."
source scripts/dev-env.sh
for tool in qtpaths6 cmake ninja rustup cargo pkg-config c++ ld.lld; do
    command -v "$tool" >/dev/null || { echo "Missing $tool; install the development prerequisites in docs/development.md." >&2; exit 1; }
done
rustup show
command -v cargo-llvm-cov >/dev/null || cargo install cargo-llvm-cov --version 0.9.1 --locked
printf 'Ubuntu Qt/KDE toolchain ready. Run scripts/verify.sh full.\n'
