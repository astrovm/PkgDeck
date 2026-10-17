#!/usr/bin/env bash
# Fail when a bundled program or library needs a newer glibc than the oldest
# system the AppImage and Snap support.
# Usage: scripts/ci/check-glibc.sh <directory> <newest allowed glibc>
set -euo pipefail
usage='Usage: scripts/ci/check-glibc.sh <directory> <newest allowed glibc>'
root=${1:?$usage}
allowed=${2:?$usage}
[[ -d $root ]] || { echo "$root is not a directory" >&2; exit 2; }
[[ $allowed =~ ^[0-9]+\.[0-9]+$ ]] || { echo "Invalid glibc version: $allowed" >&2; exit 2; }
checked=0
failed=0
while IFS= read -r -d '' path; do
    [[ $(head -c 4 "$path" | od -An -tx1 | tr -d ' \n') == 7f454c46 ]] || continue
    checked=$((checked + 1))
    newest=$({ objdump -T "$path" 2>/dev/null || true; } | { grep -o 'GLIBC_[0-9][0-9.]*' || true; } | sed 's/^GLIBC_//' | sort -uV | tail -n1)
    [[ -n $newest ]] || continue
    if [[ $(printf '%s\n%s\n' "$allowed" "$newest" | sort -V | tail -n1) != "$allowed" ]]; then
        echo "${path#"$root"/} needs glibc $newest, newer than $allowed:" >&2
        objdump -T "$path" | grep -F "(GLIBC_$newest)" >&2 || true
        failed=1
    fi
done < <(find "$root" -type f -print0)
[[ $checked -gt 0 ]] || { echo "No programs or libraries found in $root" >&2; exit 1; }
[[ $failed -eq 0 ]] || exit 1
echo "$checked programs and libraries work with glibc $allowed"
