#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."
exec "${CARGO_TARGET_DIR:-target}/debug/pkgdeck-tools" "$@"
