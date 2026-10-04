#!/usr/bin/env bash
# Keep the static CLI portable while giving its separate APT reader a private
# library closure, including glibc and its loader. No host libraries are used.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."
prefix=${1:?Usage: bundle-cli-apt.sh PREFIX}
private=$prefix/lib/pkgdeck/apt
licenses=$prefix/share/licenses/pkgdeck/apt
mkdir -p "$prefix/bin" "$private" "$licenses"
scripts/build-apt.sh "$private"
links=$(ldd "$private/pkgdeck-apt-query")
[[ $links != *'not found'* ]] || { echo "Unresolved APT dependency: $links" >&2; exit 1; }
mapfile -t libraries < <(awk '/=> \// {print $3} /^[ \t]*\// {print $1}' <<<"$links")
loader=
for source in "${libraries[@]}"; do
    name=${source##*/}
    cp -L "$source" "$private/$name"
    [[ $name != ld-linux* ]] || loader=$name
    # Preserve the copyright and license notices for every bundled library.
    owner=$(dpkg-query -S "$(realpath "$source")" 2>/dev/null || dpkg-query -S "$source")
    owner=$(awk '!/^diversion / && /: \// {print; exit}' <<<"$owner")
    [[ -n $owner ]] || { echo "No package owns $source" >&2; exit 1; }
    package=${owner%%: /*}
    package=${package%%:*}
    cp -L "/usr/share/doc/$package/copyright" "$licenses/$package"
done
[[ -n $loader ]] || { echo 'APT loader not found' >&2; exit 1; }
ln -sfn "$loader" "$private/ld.so"
# The reader folds UTF-8 metadata with glibc's C.UTF-8 locale. Hosts using
# another libc may not provide glibc locale files at all.
mkdir -p "$private/locale"
cp -a /usr/lib/locale/C.utf8 "$private/locale/"
cp -L /usr/share/doc/libc-bin/copyright "$licenses/libc-bin"
cp native/COPYING "$licenses/APT-HELPER-GPL-2"
cat > "$prefix/bin/pkgdeck-apt-query" <<'EOF'
#!/bin/sh
set -eu
root=$(CDPATH= cd -- "$(dirname -- "$0")/../lib/pkgdeck/apt" && pwd)
LOCPATH=$root/locale
export LOCPATH
exec "$root/ld.so" --library-path "$root" "$root/pkgdeck-apt-query" "$@"
EOF
chmod +x "$prefix/bin/pkgdeck-apt-query"
