#!/usr/bin/env bash
# Exercise the CLI's scan -> preview -> clean flow on a disposable home.
set -euo pipefail
[[ $(uname -s) == Darwin ]] || { echo 'Requires macOS' >&2; exit 1; }
pkd=${1:?Usage: macos-project-permissions.sh /path/to/pkd}
work=$(mktemp -d)
trap 'find "$work" -depth -delete' EXIT
profile=$work/home
config=$work/config
mkdir -p "$config/pkgdeck" "$work/bin"
# A deterministic process list avoids treating unrelated host builds as a
# build of these fixtures. All filesystem reads and deletions remain real.
printf '#!/bin/sh\nexit 0\n' > "$work/bin/ps"
chmod +x "$work/bin/ps"
for folder in code Documents Downloads Desktop 'Applications/Example.app/Contents/Resources'; do
    mkdir -p "$profile/$folder/project/node_modules"
    printf '{}\n' > "$profile/$folder/project/package.json"
    printf 'fixture\n' > "$profile/$folder/project/node_modules/data"
done
settings() {
    jq -n --arg folder "$1" '{folders:[$folder],days:0}' > "$config/pkgdeck/clean.json"
}
run() {
    env HOME="$profile" XDG_CONFIG_HOME="$config" PATH="$work/bin:/usr/bin:/bin" \
        "$pkd" --json --yes --from projects "$@"
}
settings "$profile"
run clean > "$work/list.json"
jq -e --arg key "build:$profile/code/project" '.exit_code == 0 and (.data.items | length == 1) and .data.items[0].id.key == $key' "$work/list.json" >/dev/null
if run clean "projects:build:$profile/Downloads/project" > "$work/stale.json"; then
    echo 'An unselected protected folder accepted a stale cleanup key' >&2
    exit 1
fi
run clean --all > "$work/clean.json"
jq -e '.exit_code == 0' "$work/clean.json" >/dev/null
test ! -e "$profile/code/project/node_modules"
for folder in Documents Downloads Desktop 'Applications/Example.app/Contents/Resources'; do
    test -f "$profile/$folder/project/node_modules/data"
done
settings "$profile/Downloads"
run clean > "$work/selected.json"
jq -e '.exit_code == 0 and (.data.items | length == 1)' "$work/selected.json" >/dev/null
run clean --all > "$work/selected-clean.json"
jq -e '.exit_code == 0' "$work/selected-clean.json" >/dev/null
test ! -e "$profile/Downloads/project/node_modules"
test -f "$profile/Downloads/project/package.json"
settings "$profile/Applications/Example.app/Contents/Resources"
run clean > "$work/app.json"
jq -e '.exit_code == 0 and (.data.items | length == 0)' "$work/app.json" >/dev/null
test -f "$profile/Applications/Example.app/Contents/Resources/project/node_modules/data"
echo 'PASS broad scan, stale key refusal, selected-folder cleanup and app-bundle protection'
