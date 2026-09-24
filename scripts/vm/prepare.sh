#!/usr/bin/env bash
# Dependencies only; allowed exclusively in disposable containers.
set -euo pipefail
[[ ${1:-} == --container && $EUID == 0 && -f /etc/pkgdeck-disposable-container &&
    (-f /run/.containerenv || ${container:-} == podman) ]]
apt-get update -qq
apt-get install -y --no-install-recommends pkexec polkitd sudo libapt-pkg-dev build-essential git curl jq file procps ca-certificates
useradd -m linuxbrew
checkout=/home/linuxbrew/.linuxbrew/Homebrew
mkdir -p "$checkout" /home/linuxbrew/.linuxbrew/bin
archive=/opt/pkgdeck-brew-7.0.6.tar.gz
curl --fail --location --retry 3 --max-time 120 https://github.com/Homebrew/brew/archive/refs/tags/7.0.6.tar.gz -o "$archive"
echo "407a9e64850bc31247371571c7bcc87ea157780390998cccba473b030ad6b7ae  $archive" | sha256sum -c -
tar -xzf "$archive" --strip-components=1 -C "$checkout"
ln -s ../Homebrew/bin/brew /home/linuxbrew/.linuxbrew/bin/brew
git -C "$checkout" init -b stable
git -C "$checkout" add .
git -C "$checkout" -c user.name=Synthetic -c user.email=fixture@example.invalid commit -qm 'Pinned Homebrew 7.0.6 fixture'
git -C "$checkout" tag 7.0.6
git -C "$checkout" branch main
git clone --bare "$checkout" /opt/brew-origin.git
git -C "$checkout" remote add origin /opt/brew-origin.git
git -C "$checkout" fetch origin
chown -R linuxbrew:linuxbrew /home/linuxbrew /opt/brew-origin.git
runuser -u linuxbrew -- env HOMEBREW_NO_AUTO_UPDATE=1 HOMEBREW_NO_ANALYTICS=1 /home/linuxbrew/.linuxbrew/bin/brew ruby -e 'puts RUBY_VERSION' >/dev/null
runuser -u linuxbrew -- env HOMEBREW_NO_AUTO_UPDATE=1 HOMEBREW_NO_ANALYTICS=1 /home/linuxbrew/.linuxbrew/bin/brew formulae >/dev/null
dpkg-query -W >/opt/pkgdeck-os-packages.txt
echo dependencies-only >/etc/pkgdeck-prepared
echo PKGDECK_PREPARE_PASS
