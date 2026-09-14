#!/usr/bin/env bash
# Guard project-owned entrypoints; host sanitization intentionally names PYTHONPATH.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."
if git ls-files --cached --others --exclude-standard | while IFS= read -r path; do
    [[ -f $path ]] || continue
    [[ $path != scripts/check-no-python.sh ]] || continue
    case $path in
        *.py | *requirements*.txt | *Pipfile* | *pyproject.toml) echo "$path" ;;
        scripts/* | containers/* | .github/* | crates/*)
            if grep -En '(^#!.*python|Command::new\("python|python3[[:space:]]|python3-apt|setup-python@|aqtinstall)' "$path"; then echo "$path"; fi
            ;;
    esac
done | grep .; then
    echo 'Project Python dependency found' >&2
    exit 1
fi
