#!/usr/bin/env bash
# Use GitHub's Azure Ubuntu mirror first, then Canonical's when it times out
# or refuses connections. Run as root before the first apt-get update.
set -euo pipefail
sources=/etc/apt/sources.list.d/ubuntu.sources
[[ -f $sources ]] || exit 0
case "$(dpkg --print-architecture)" in
amd64 | i386) official=http://archive.ubuntu.com/ubuntu/ ;;
*) official=http://ports.ubuntu.com/ubuntu-ports/ ;;
esac
printf '%s\n' http://azure.archive.ubuntu.com/ubuntu/ "$official" >/etc/apt/ubuntu-mirrors.txt
sed -i -E 's#^URIs: .*#URIs: mirror+file:/etc/apt/ubuntu-mirrors.txt#' "$sources"
printf 'Acquire::Retries "5";\nAcquire::http::Timeout "30";\n' >/etc/apt/apt.conf.d/80-pkgdeck-mirrors
