#!/usr/bin/env bash
# Use GitHub's Azure Ubuntu mirror first, then Canonical's when it times out
# or refuses connections. Run as root before the first apt-get update.
# Ubuntu 24.04 and newer list their sources in ubuntu.sources. Ubuntu 22.04,
# which the portable container uses, still lists them in sources.list.
set -euo pipefail
sources=/etc/apt/sources.list.d/ubuntu.sources
list=/etc/apt/sources.list
mirrors=mirror+file:/etc/apt/ubuntu-mirrors.txt
if [[ -f $sources ]]; then
    format=deb822
elif [[ -f $list ]] && grep -Eq '^deb .*://(archive|security|ports)\.ubuntu\.com/' "$list"; then
    format=list
else
    exit 0
fi
case "$(dpkg --print-architecture)" in
amd64 | i386) official=http://archive.ubuntu.com/ubuntu/ ;;
*) official=http://ports.ubuntu.com/ubuntu-ports/ ;;
esac
printf '%s\n' http://azure.archive.ubuntu.com/ubuntu/ "$official" >/etc/apt/ubuntu-mirrors.txt
if [[ $format == deb822 ]]; then
    sed -i -E "s#^URIs: .*#URIs: $mirrors#" "$sources"
else
    sed -i -E "s#https?://(archive|security)\\.ubuntu\\.com/ubuntu/?#$mirrors#; s#https?://ports\\.ubuntu\\.com/ubuntu-ports/?#$mirrors#" "$list"
fi
printf 'Acquire::Retries "5";\nAcquire::http::Timeout "30";\n' >/etc/apt/apt.conf.d/80-pkgdeck-mirrors
