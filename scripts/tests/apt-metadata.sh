#!/usr/bin/env bash
# APT uses only synthetic metadata under this temporary root; no host writes.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/../.."
helper=$(realpath "${CARGO_TARGET_DIR:-target}/debug/pkgdeck-apt-query")
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
mkdir -p "$work/"{etc/apt,repo,var/lib/apt/lists/partial,var/lib/dpkg,var/cache/apt/archives/partial}
cat >"$work/config" <<CONFIG
Dir "$work";
Dir::State "var/lib/apt";
Dir::State::status "$work/var/lib/dpkg/status";
APT::Architecture "amd64";
APT::Architectures { "amd64"; "arm64"; };
Dir::Cache::pkgcache "$work/var/cache/apt/pkgcache.bin";
Dir::Cache::srcpkgcache "$work/var/cache/apt/srcpkgcache.bin";
APT::Sandbox::User "$(id -un)";
CONFIG
cat >"$work/var/lib/dpkg/status" <<'STATUS'
Package: pkgdeck-fixture
Status: install ok installed
Architecture: amd64
Version: 1.0
Description: Synthetic installed fixture

STATUS
for version in 1.0 2.0; do
    cat >>"$work/repo/Packages" <<PACKAGE
Package: pkgdeck-fixture
Architecture: amd64
Version: $version
Description: Synthetic "quoted" café fixture
 Long description with a backslash \\ and a newline.
 .
 Second paragraph.
Homepage: https://example.invalid/pkgdeck
Depends: synthetic-dependency (>= 1)
Filename: fixture-$version.deb
Size: 1

PACKAGE
done
cat >>"$work/repo/Packages" <<'PACKAGE'
Package: pkgdeck-fixture
Architecture: arm64
Version: 3.0
Description: Synthetic foreign architecture
Filename: fixture-arm64.deb
Size: 1

PACKAGE
printf 'deb [trusted=yes] file:%s/repo ./\n' "$work" >"$work/etc/apt/sources.list"
APT_CONFIG="$work/config" apt-get update -qq
rm -f "$work/var/cache/apt/"*.bin
query() { APT_CONFIG="$work/config" "$helper" "$@"; }
query detect '' '' | jq -e '.==[]'
query details pkgdeck-fixture amd64 | jq -e 'length==1 and .[0].package.installed_version=="1.0" and .[0].package.candidate_version=="2.0" and .[0].package.update=="available" and .[0].description=="Synthetic \"quoted\" café fixture\nLong description with a backslash \\ and a newline.\n\nSecond paragraph." and .[0].homepage=="https://example.invalid/pkgdeck" and .[0].dependencies==["synthetic-dependency (>= 1)"]'
query details pkgdeck-fixture arm64 | jq -e 'length==1 and .[0].package.id.architecture=="arm64" and .[0].package.installed_version==null'
query search QUOTED '' | jq -e 'length==1'
query search CAFÉ '' | jq -e 'length==1'
query installed '' '' | jq -e 'length==1'
cat >"$work/etc/apt/preferences" <<'PINS'
Package: pkgdeck-fixture
Pin: version 1.0
Pin-Priority: 1001
PINS
query details pkgdeck-fixture amd64 | jq -e '.[0].package.candidate_version=="1.0" and .[0].package.update=="current"'
# Phase eligibility comes from APT itself, including administrator overrides.
rm "$work/etc/apt/preferences"
sed -i '/^Version: 2.0$/a Phased-Update-Percentage: 0' "$work/repo/Packages"
rm -f "$work/var/lib/apt/lists/"*Packages*
APT_CONFIG="$work/config" apt-get update -qq
rm -f "$work/var/cache/apt/"*.bin
printf '\nAPT::Get::Never-Include-Phased-Updates "true";\n' >>"$work/config"
query installed '' '' | jq -e '.[0].package.candidate_version=="2.0" and .[0].package.update=="current"'
printf '\nAPT::Get::Always-Include-Phased-Updates "true";\n' >>"$work/config"
query installed '' '' | jq -e '.[0].package.update=="available"'
sed -i 's/^Status: install ok installed$/Status: hold ok installed/' "$work/var/lib/dpkg/status"
query installed '' '' | jq -e '.[0].package.update=="current"'
[[ ! -e $work/var/cache/apt/pkgcache.bin && ! -e $work/var/cache/apt/srcpkgcache.bin ]]
echo 'PASS native APT candidates, pinning, phasing, holds, multiarch, installed state and JSON escaping'
