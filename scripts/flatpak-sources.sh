#!/usr/bin/env bash
# Regenerate Flatpak's offline crates.io sources from Cargo's generated lockfile.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."
case "${1:-}" in
    ''|--check) ;;
    *) echo 'Usage: scripts/flatpak-sources.sh [--check]' >&2; exit 2 ;;
esac
generated=$(mktemp)
trap 'rm -f "$generated"' EXIT
jq -n --indent 4 --rawfile lock Cargo.lock '
    [$lock | split("[[package]]")[1:][] |
        [scan("(?m)^(name|version|source|checksum) = \"([^\"]+)\"$")] |
        map({key: .[0], value: .[1]}) | from_entries |
        select(.source != null) |
        if .source != "registry+https://github.com/rust-lang/crates.io-index" or
           (.checksum | test("^[0-9a-f]{64}$") | not)
        then error("Unsupported Cargo source or checksum") else . end |
        . as $crate | "cargo/vendor/\(.name)-\(.version)" as $dest |
        {type: "archive", "archive-type": "tar-gzip",
         url: "https://static.crates.io/crates/\(.name)/\(.name)-\(.version).crate",
         sha256: .checksum, dest: $dest},
        {type: "inline",
         contents: "{\"package\": \"\($crate.checksum)\", \"files\": {}}",
         dest: $dest, "dest-filename": ".cargo-checksum.json"}
    ] + [{type: "inline",
          contents: "[source.vendored-sources]\ndirectory = \"cargo/vendor\"\n\n[source.crates-io]\nreplace-with = \"vendored-sources\"\n",
          dest: "cargo", "dest-filename": "config.toml"}]
' >"$generated"
manifest=packaging/flatpak/cargo-sources.json
if [[ ${1:-} == --check ]]; then
    cmp -s "$generated" "$manifest" || {
        echo 'Flatpak sources are stale. Run scripts/flatpak-sources.sh and commit the result.' >&2
        exit 1
    }
else
    cp "$generated" "$manifest"
fi
