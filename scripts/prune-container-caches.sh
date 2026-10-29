#!/usr/bin/env bash
# Deletes development container caches nothing uses any more, so they don't
# pile up: in this checkout, the caches of older container recipes once
# they've gone a day unused; in other checkouts, every cache once the
# checkout is gone or two weeks have passed since a run used it.
# Usage: prune-container-caches.sh CACHES WORKSPACE_KEY CURRENT_KEY WORKSPACE_PATH
set -euo pipefail
caches=${1:?}
workspace_key=${2:?}
current=${3:?}
workspace_path=${4:?}
mine="$caches/$workspace_key"
mkdir -p "$mine/$current"
# A run marks the cache it uses, and its checkout says where it lives.
touch "$mine/$current/.last-used"
printf '%s\n' "$workspace_path" >"$mine/workspace"
# The last time a run used `dir`, in seconds since the epoch.
used() {
    local dir=$1 newest=0 stamp
    for stamp in "$dir/.last-used" "$dir"/*/.last-used; do
        [[ -e $stamp ]] || continue
        local time
        time=$(stat -c %Y "$stamp" 2>/dev/null || stat -f %m "$stamp")
        ((time > newest)) && newest=$time
    done
    if ((newest == 0)); then
        newest=$(stat -c %Y "$dir" 2>/dev/null || stat -f %m "$dir")
    fi
    printf '%s\n' "$newest"
}
now=$(date +%s)
day=86400
for dir in "$mine"/*/; do
    dir=${dir%/}
    [[ ${dir##*/} == "$current" ]] && continue
    if (($(used "$dir") < now - day)); then
        echo "Deleting an old container cache: $dir" >&2
        rm -rf "$dir"
    fi
done
for dir in "$caches"/*/; do
    dir=${dir%/}
    [[ $dir == "$mine" ]] && continue
    path=$(cat "$dir/workspace" 2>/dev/null || true)
    if [[ -n $path && ! -d $path ]] || (($(used "$dir") < now - 14 * day)); then
        echo "Deleting the container caches of another checkout: $dir" >&2
        rm -rf "$dir"
    fi
done
