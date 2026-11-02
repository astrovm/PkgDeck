#!/usr/bin/env bash
# Pull a container image, trying again when the registry fails: Docker Hub
# answers 404 for a blob now and then, and that's not what the tests check.
#   scripts/tests/pull-image.sh ENGINE IMAGE [PULL OPTIONS...]
set -euo pipefail
engine=${1:?Usage: pull-image.sh ENGINE IMAGE [OPTIONS...]}
image=${2:?Usage: pull-image.sh ENGINE IMAGE [OPTIONS...]}
shift 2
for attempt in 1 2 3; do
    if "$engine" pull -q "$@" "$image" >/dev/null; then exit 0; fi
    echo "Pulling $image failed (try $attempt of 3)" >&2
    [[ $attempt == 3 ]] || sleep 15
done
exit 1
