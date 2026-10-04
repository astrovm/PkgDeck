#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/../.."
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
mkdir "$work/bin"
cat > "$work/bin/brew" <<'EOF'
#!/bin/bash
[[ $* == 'style --cask synthetic/test/pkgdeck' ]] || exit 90
count=0
[[ ! -f $STYLE_CALLS ]] || read -r count < "$STYLE_CALLS"
echo "$((count + 1))" > "$STYLE_CALLS"
case "$STYLE_CASE" in
    transient) ((count >= 2)) && exit 0; echo 'Network error while fetching https://rubygems.org/spec'; exit 7 ;;
    persistent) echo 'Error: failed to run bundle install'; exit 8 ;;
    violation) echo '1 file inspected, 1 offense detected'; exit 9 ;;
esac
EOF
printf '#!/bin/sh\nexit 0\n' > "$work/bin/sleep"
chmod +x "$work/bin/"*
for mode in transient persistent violation; do
    rm -f "$work/calls"
    status=0
    PATH="$work/bin:$PATH" STYLE_CALLS="$work/calls" STYLE_CASE="$mode" \
        scripts/homebrew-style.sh --cask synthetic/test/pkgdeck > "$work/output" 2>&1 || status=$?
    read -r calls < "$work/calls"
    case "$mode" in
        transient) [[ $status == 0 && $calls == 3 ]] ;;
        persistent) [[ $status == 8 && $calls == 3 ]] ;;
        violation) [[ $status == 9 && $calls == 1 ]] ;;
    esac
done
echo 'PASS Homebrew style bootstrap retries, bounded failures and immediate style violations'
