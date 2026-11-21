#!/usr/bin/env bash
# Removes the coverage build folder when another rustc built it.
#
# Cargo 1.100 builds into a new folder layout, and the clean that cargo
# llvm-cov runs before it measures removes files only in the current layout.
# Test binaries that an older toolchain left in the old layout then join the
# report. Their counters are all zero, so lines that the tests run show as not
# covered. A new rustc rebuilds everything, so the removal costs no build time.
#
# Usage: scripts/coverage-reset.sh [FOLDER]
# FOLDER is the folder cargo llvm-cov builds in: $CARGO_LLVM_COV_TARGET_DIR,
# or llvm-cov-target in Cargo's target folder.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."
folder=${1:-${CARGO_LLVM_COV_TARGET_DIR:-$(cargo metadata --format-version 1 --no-deps | jq -r .target_directory)/llvm-cov-target}}
# Cargo keeps the version of the rustc that built a folder in .rustc_info.json.
built_by=$(jq -r '[.outputs[].stdout | select(startswith("rustc "))][0] // "" | split("\n")[0]' "$folder/.rustc_info.json" 2>/dev/null || true)
current=$(rustc -V)
if [[ -n $built_by && $built_by != "$current" ]]; then
    printf 'Removing %s: %s built it, and this build uses %s\n' "$folder" "$built_by" "$current"
    rm -rf "$folder"
fi
