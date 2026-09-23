#!/usr/bin/env bash
# Sourced after the disposable container or hosted-runner guard.
set -euo pipefail
cli() {
    local source=$1 user
    shift
    if [[ $source == apt ]]; then user=pkgdeck-test; else user=linuxbrew; fi
    runuser -u "$user" -- env PATH=/home/linuxbrew/.linuxbrew/bin:/usr/bin:/bin /opt/pkgdeck-bin/pkd --json --yes --from "$source" "$@" |
        jq -e 'if .schema_version == 1 and .exit_code == 0 then .data else error("CLI failure") end'
}
write() {
    local source=$1 operation=$2 name=${3:-} user
    if [[ $source == apt ]]; then user=pkgdeck-test; else user=linuxbrew; fi
    echo "LIFECYCLE ${PKGDECK_FRONTEND:-cli} $source $operation $name"
    case ${PKGDECK_FRONTEND:-cli} in
        gui) /mnt/pkgdeck-tools gui-write "$operation" "$name" runuser -u "$user" -- env -u XDG_CONFIG_HOME PATH=/home/linuxbrew/.linuxbrew/bin:/usr/bin:/bin /mnt/pkgdeck-app/AppRun --from "$source" --auth sudo ;;
        cli) if [[ -n $name ]]; then cli "$source" "$operation" "$name"; else cli "$source" "$operation"; fi ;;
        *) echo "Unknown frontend: $PKGDECK_FRONTEND (expected cli or gui)" >&2; return 2 ;;
    esac
}
apt_version() {
    local version=$1 package=/tmp/pkgdeck-package repo=/opt/pkgdeck-fixture-repo
    mkdir -p "$package/DEBIAN" "$package/usr/share/pkgdeck-fixture" "$repo"
    printf '%s\n' "$version" >"$package/usr/share/pkgdeck-fixture/version"
    printf 'Package: pkgdeck-fixture\nVersion: %s\nArchitecture: all\nMaintainer: Synthetic <fixture@example.invalid>\nDescription: Disposable host execution fixture\n' "$version" >"$package/DEBIAN/control"
    local deb="$repo/pkgdeck-fixture_${version}_all.deb"
    dpkg-deb --build --root-owner-group "$package" "$deb"
    {
        cat "$package/DEBIAN/control"
        printf 'Filename: %s\nSize: %s\nSHA256: %s\n\n' "${deb##*/}" "$(stat -c %s "$deb")" "$(sha256sum "$deb" | cut -d ' ' -f1)"
    } >"$repo/Packages"
}
apt_fixture() {
    apt_version 1.0
    echo 'deb [trusted=yes] file:/opt/pkgdeck-fixture-repo ./' >/etc/apt/sources.list.d/pkgdeck-fixture.list
    apt-get update -qq -o Dir::Etc::sourcelist=/etc/apt/sources.list.d/pkgdeck-fixture.list -o Dir::Etc::sourceparts=-
}
apt_lifecycle() {
    cli apt sources | jq -e '.sources[0].availability == {Ok:"available"}'
    cli apt search pkgdeck-fixture | jq -e 'any(.packages[]; .id.name=="pkgdeck-fixture")'
    cli apt info pkgdeck-fixture | jq -e '.package.candidate_version=="1.0"'
    write apt install pkgdeck-fixture
    [[ $(dpkg-query -W '-f=${Version}' pkgdeck-fixture) == 1.0 ]]
    apt_version 2.0
    write apt update
    cli apt list | jq -e 'any(.packages[]; .id.name=="pkgdeck-fixture" and .installed_version=="1.0" and .candidate_version=="2.0" and .update=="available")'
    write apt upgrade pkgdeck-fixture
    [[ $(dpkg-query -W '-f=${Version}' pkgdeck-fixture) == 2.0 && $(cat /usr/share/pkgdeck-fixture/version) == 2.0 ]]
    write apt remove pkgdeck-fixture
    [[ ! -e /usr/share/pkgdeck-fixture/version ]]
    cli apt list | jq -e 'all(.packages[]; .id.name!="pkgdeck-fixture")'
    echo 'PASS package lifecycle APT detect/search/details/install/update/upgrade/remove 1.0 → 2.0'
}
brew() { runuser -u linuxbrew -- env HOMEBREW_NO_AUTO_UPDATE=1 HOMEBREW_NO_ANALYTICS=1 /home/linuxbrew/.linuxbrew/bin/brew "$@"; }
brew_version() {
    local number=$1 archive=/opt/pkgdeck-fixture-$1.tar.gz
    mkdir -p /tmp/pkgdeck-brew-source /opt/fixture-tap/Formula
    printf '#!/bin/sh\nprintf "%s\\n"\n' "$number" >/tmp/pkgdeck-brew-source/pkgdeck-fixture
    chmod +x /tmp/pkgdeck-brew-source/pkgdeck-fixture
    tar -czf "$archive" -C /tmp/pkgdeck-brew-source pkgdeck-fixture
    cat >/opt/fixture-tap/Formula/pkgdeck-fixture.rb <<RUBY
class PkgdeckFixture < Formula
  desc "Synthetic PkgDeck lifecycle fixture"
  homepage "https://example.invalid/pkgdeck"
  url "file://$archive"
  version "$number"
  sha256 "$(sha256sum "$archive" | cut -d ' ' -f1)"
  license "MIT"
  def install
    bin.install "pkgdeck-fixture"
  end
end
RUBY
    git -c safe.directory=/opt/fixture-tap -C /opt/fixture-tap add .
    git -c safe.directory=/opt/fixture-tap -C /opt/fixture-tap -c user.name=Synthetic -c user.email=fixture@example.invalid commit -qm "Fixture $number"
    chown -R linuxbrew:linuxbrew /opt/fixture-tap
}
brew_lifecycle() {
    mkdir -p /opt/fixture-tap
    git -C /opt/fixture-tap init -b main
    brew_version 1.0
    brew tap pkgdeck/fixtures /opt/fixture-tap
    brew trust --tap pkgdeck/fixtures
    local name=pkgdeck/fixtures/pkgdeck-fixture
    cli homebrew sources | jq -e '.sources[0].availability=={Ok:"available"}'
    cli homebrew search pkgdeck-fixture | jq -e --arg n "$name" 'any(.packages[];.id.name==$n)'
    cli homebrew info "$name" | jq -e '.package.candidate_version=="1.0"'
    write homebrew install "$name"
    brew info --json=v2 "$name" | jq -e '.formulae[0].installed[0].version=="1.0"'
    brew_version 2.0
    write homebrew update
    cli homebrew list | jq -e --arg n "$name" 'any(.packages[];.id.name==$n and .installed_version=="1.0" and .candidate_version=="2.0" and .update=="available")'
    write homebrew upgrade "$name"
    brew info --json=v2 "$name" | jq -e '[.formulae[0].installed[].version] | contains(["1.0","2.0"])'
    cli homebrew info "$name" | jq -e '.package.installed_version=="2.0"'
    [[ $(/home/linuxbrew/.linuxbrew/bin/pkgdeck-fixture) == 2.0 ]]
    write homebrew remove "$name"
    [[ ! -e /home/linuxbrew/.linuxbrew/bin/pkgdeck-fixture ]]
    brew info --json=v2 "$name" | jq -e '.formulae[0].installed==[]'
    echo 'PASS package lifecycle Homebrew detect/search/details/install/update/upgrade/remove 1.0 → 2.0'
}
