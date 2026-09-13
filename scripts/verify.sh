#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."
mode=${1:-fast}
if (($#)); then shift; fi
if [[ "$mode" == --help ]]; then
    echo 'Usage: scripts/verify.sh [fast|full|vm]'
    exit 0
fi
if (($#)) || [[ ! "$mode" =~ ^(fast|full|vm)$ ]]; then
    echo 'Usage: scripts/verify.sh [fast|full|vm]' >&2
    exit 2
fi
for tool in cargo python3 timeout tee; do
    command -v "$tool" >/dev/null || { echo "Missing prerequisite: $tool" >&2; exit 1; }
done
log_dir="${PKGDECK_LOG_ROOT:-$PWD/build/verification}/$(date -u +%Y%m%dT%H%M%SZ)-$$-$mode"
mkdir -p "$log_dir"
ln -sfn "$(basename "$log_dir")" "$(dirname "$log_dir")/latest"
printf 'Verification logs: %s\n' "$log_dir"
stage() {
    local name=$1; shift
    printf '\nRunning %s\n' "$name"
    if timeout --kill-after=10s "${PKGDECK_STAGE_TIMEOUT:-20m}" "$@" 2>&1 | tee "$log_dir/$name.log"; then
        printf 'PASS %s\n' "$name"
    else
        local code=$?
        printf 'FAIL %s (exit %s). Log: %s/%s.log\n' "$name" "$code" "$log_dir" "$name" >&2
        exit "$code"
    fi
}
if [[ "$mode" == vm ]]; then
    stage build-probe cargo build --locked -p pkgdeck-core --example apt-probe
    stage build-cli cargo build --locked -p pkd
    stage vm python3 -u scripts/test-host-vm.py
    exit 0
fi
stage format cargo fmt --all --check
stage infrastructure python3 -m unittest discover -s scripts/tests -v
if [[ "$mode" == fast ]]; then
    stage lint cargo clippy --locked -p pkgdeck-core -p pkd --all-targets -- -D warnings
    stage tests cargo test --locked -p pkgdeck-core -p pkd
    stage build cargo build --locked -p pkd
    stage qt-free python3 scripts/check-qt-free.py
else
    source scripts/dev-env.sh
    [[ -x "$QT_ROOT_DIR/bin/qtpaths" ]] && [[ $("$QT_ROOT_DIR/bin/qtpaths" --qt-version) == 6.11.2 ]] && [[ -f "$PKGDECK_SDK_PREFIX/lib/cmake/KF6Kirigami/KF6KirigamiConfig.cmake" ]] || {
        echo 'Pinned Qt 6.11.2/Kirigami SDK missing. Run scripts/setup-dev.sh or set QT_ROOT_DIR and PKGDECK_SDK_PREFIX.' >&2; exit 1;
    }
    command -v cargo-llvm-cov >/dev/null && [[ $(cargo llvm-cov --version) == 'cargo-llvm-cov 0.9.1' ]] || {
        echo 'cargo-llvm-cov 0.9.1 missing. Run scripts/setup-dev.sh.' >&2; exit 1;
    }
    export QT_QPA_PLATFORM=offscreen QT_QUICK_BACKEND=software
    stage lint cargo clippy --workspace --all-targets --locked -- -D warnings
    mkdir -p coverage
    stage coverage cargo llvm-cov --workspace --include-build-script --locked --fail-under-lines 95 --lcov --output-path coverage/lcov.info
    stage release cargo build --workspace --release --locked
fi
