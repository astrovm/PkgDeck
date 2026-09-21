#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."
mode=${1:-fast}
if (($#)); then shift; fi
usage='Usage: scripts/verify.sh [fast|full|vm|containers] [--engine native|podman] [--only lint,coverage,release,tests]'
if [[ "$mode" == --help ]]; then echo "$usage"; exit 0; fi
engine=native
only=all
while (($#)); do
    case "${1:-}" in
        --engine) [[ $# -ge 2 ]] || { echo "$usage" >&2; exit 2; }; engine=$2; shift 2;;
        --only) [[ $# -ge 2 ]] || { echo "$usage" >&2; exit 2; }; only=$2; shift 2;;
        *) echo "$usage" >&2; exit 2;;
    esac
done
if [[ ! "$mode" =~ ^(fast|full|vm|containers)$ ]] || [[ ! "$engine" =~ ^(native|podman)$ ]] || [[ "$mode" == vm && "$engine" == podman ]]; then
    echo "$usage" >&2
    exit 2
fi
if [[ "$mode" != full && "$only" != all ]]; then
    echo "--only applies to full mode" >&2
    exit 2
fi
if [[ ! "$only" =~ ^(all|(lint|coverage|release|tests)(,(lint|coverage|release|tests))*)$ ]]; then
    echo "$usage" >&2
    exit 2
fi
# Full-mode stage selector for parallel CI jobs sharing one rust cache key.
want() { [[ $only == all || ",$only," == *",$1,"* ]]; }
prerequisites=(timeout tee)
if [[ "$engine" == native ]]; then prerequisites+=(cargo jq); fi
for tool in "${prerequisites[@]}"; do
    command -v "$tool" >/dev/null || { echo "Missing prerequisite: $tool" >&2; exit 1; }
done
log_root="${PKGDECK_LOG_ROOT:-$PWD/build/verification}"
mkdir -p "$log_root"
log_dir=$(mktemp -d "$log_root/$(date -u +%Y%m%dT%H%M%SZ)-$mode-XXXXXX")
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
if [[ "$mode" == containers ]]; then
    if [[ "$engine" == podman ]]; then
        stage build-cli scripts/container.sh development --build-cli
        PKGDECK_BINARY_DIR=$(scripts/container.sh development --binary-dir)
        PKGDECK_TOOLS_BINARY="$PKGDECK_BINARY_DIR/pkgdeck-tools"
        export PKGDECK_TOOLS_BINARY
        export PKGDECK_BINARY_DIR
    else
        stage build-cli cargo build --locked -p pkd -p pkgdeck-tools
    fi
    stage test-cli-lifecycle scripts/container.sh lifecycle
    exit 0
fi
if [[ "$engine" == podman ]]; then
    stage "verify-podman-$mode" scripts/container.sh development "$mode" --only "$only"
    exit 0
fi
if [[ "$mode" == vm ]]; then
    stage build-probe cargo build --locked -p pkgdeck-core --example apt-probe
    stage build-cli cargo build --locked -p pkd -p pkgdeck-tools
    stage test-vm-lifecycle scripts/test-host-vm.sh
    exit 0
fi
# Stage-selected full runs (parallel CI jobs) skip the shared prefix: the
# terminal job already covers it, and repeating it in every split job would
# cost more than it signals.
if [[ $only == all ]]; then
    stage format cargo fmt --all --check
    stage test-infrastructure scripts/tests/infrastructure.sh
    stage check-no-python scripts/check-no-python.sh
    stage build-apt-helper scripts/build-apt.sh
    stage test-apt-metadata scripts/tests/apt-metadata.sh
fi
if [[ "$mode" == fast ]]; then
    stage lint-terminal cargo clippy --locked -p pkgdeck-core -p pkd --all-targets -- -D warnings
    stage test-terminal cargo test --locked -p pkgdeck-core -p pkd
    stage build-terminal cargo build --locked -p pkd -p pkgdeck-tools
    stage check-qt-free scripts/check-qt-free.sh
else
    source scripts/dev-env.sh
    [[ $(qtpaths6 --qt-version) == 6.10.2 ]] && dpkg-query -W libkirigami-dev >/dev/null 2>&1 || {
        echo 'Ubuntu Qt 6.10.2/Kirigami toolchain missing. Install the development prerequisites.' >&2; exit 1;
    }
    if want coverage; then
        command -v cargo-llvm-cov >/dev/null && [[ $(cargo llvm-cov --version) == 'cargo-llvm-cov 0.9.1' ]] || {
            echo 'cargo-llvm-cov 0.9.1 missing. Run scripts/setup-dev.sh.' >&2; exit 1;
        }
    fi
    export QT_QPA_PLATFORM=offscreen QT_QUICK_BACKEND=software
    want lint && stage lint-workspace cargo clippy --workspace --all-targets --locked -- -D warnings
    if want coverage; then
        mkdir -p coverage
        stage test-coverage cargo llvm-cov --workspace --include-build-script --ignore-filename-regex pkgdeck-tools --locked --lcov --output-path coverage/lcov.info
        stage check-coverage cargo llvm-cov report --summary-only --include-build-script --ignore-filename-regex pkgdeck-tools --fail-under-lines 95
    fi
    want release && stage build-release cargo build --workspace --release --locked
    # Plain debug test run without coverage instrumentation; the aarch64 CI
    # job uses this while x86_64 carries the coverage gate.
    want tests && stage test-workspace cargo test --workspace --locked
    # A skipped trailing stage leaves a non-zero status behind; reaching
    # this point means every selected stage passed.
    exit 0
fi
