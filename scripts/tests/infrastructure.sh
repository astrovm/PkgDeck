#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/../.."
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
expect_code() {
    local expected=$1
    shift
    local actual=0
    "$@" >"$work/output" 2>&1 || actual=$?
    [[ $actual == "$expected" ]] || {
        cat "$work/output"
        echo "Expected $expected, got $actual"
        exit 1
    }
}
for args in --help unknown 'fast --unknown'; do
    expected=2
    [[ $args != --help ]] || expected=0
    read -ra words <<<"$args"
    expect_code "$expected" scripts/verify.sh "${words[@]}"
    grep -q Usage: "$work/output"
done
mkdir "$work/bin"
printf '#!/bin/sh\necho synthetic-format-error\nexit 23\n' >"$work/bin/cargo"
chmod +x "$work/bin/cargo"
expect_code 23 env PATH="$work/bin:$PATH" PKGDECK_LOG_ROOT="$work/logs" scripts/verify.sh fast
grep -q 'FAIL format' "$work/output"
grep -q synthetic-format-error "$work/logs/latest/format.log"
[[ ! -e $work/logs/latest/tests.log ]]
printf '#!/bin/sh\necho synthetic-slow-stage\nexec sleep 30\n' >"$work/bin/cargo"
expect_code 124 env PATH="$work/bin:$PATH" PKGDECK_LOG_ROOT="$work/slow" PKGDECK_STAGE_TIMEOUT=0.1s scripts/verify.sh fast
grep -q synthetic-slow-stage "$work/slow/latest/format.log"
source scripts/vm/cache.sh
printf synthetic-base >"$work/base"
qemu-img() { printf synthetic-prepared >"${@: -2:1}"; }
boot() {
    echo prepare >>"$work/calls"
    [[ ! -e $work/fail ]]
}
: >"$work/fail"
expect_code 1 prepare_cached "$work/base" "$work/prepared" "$work/sha"
[[ ! -e $work/prepared && ! -e $work/prepared.part && ! -e $work/sha ]]
rm "$work/fail" "$work/calls"
prepare_cached "$work/base" "$work/prepared" "$work/sha"
prepare_cached "$work/base" "$work/prepared" "$work/sha"
[[ $(wc -l <"$work/calls") == 1 ]]
printf corrupted >"$work/prepared"
prepare_cached "$work/base" "$work/prepared" "$work/sha"
[[ $(wc -l <"$work/calls") == 2 && ! -e $work/prepared.part ]]
echo 'PASS verifier arguments, failure, timeout; VM cache publication, reuse and corruption'
