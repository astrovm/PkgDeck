#!/usr/bin/env bash
# Combine line coverage from several platforms into one number.
#
# Each platform's report (lcov, from cargo llvm-cov) sees only the code built
# there: Linux never compiles the macOS-only code and the other way around. A
# line counts once, and is covered when any platform's tests ran it. Paths are
# made relative to the repository, since each runner checks out elsewhere.
#
# Usage: scripts/coverage-merge.sh [--fail-under PCT] [--summary FILE] LABEL=LCOV ...
set -euo pipefail
fail_under=
summary=/dev/null
reports=()
while (($#)); do
    case $1 in
    --fail-under) fail_under=${2:?--fail-under needs a percentage}; shift 2 ;;
    --summary) summary=${2:?--summary needs a file}; shift 2 ;;
    *=*) reports+=("$1"); shift ;;
    *) echo "Expected LABEL=LCOV, got $1" >&2; exit 2 ;;
    esac
done
((${#reports[@]})) || { echo 'Usage: coverage-merge.sh [--fail-under PCT] [--summary FILE] LABEL=LCOV ...' >&2; exit 2; }

# One "LABEL<TAB>relative/file:line<TAB>hits" record per line of every report.
records() {
    local spec label path
    for spec in "${reports[@]}"; do
        label=${spec%%=*}
        path=${spec#*=}
        [[ -r $path ]] || { echo "Missing coverage report: $path" >&2; exit 1; }
        awk -v label="$label" '
            /^SF:/ { file = substr($0, 4); sub(/^.*\/crates\//, "crates/", file) }
            /^DA:/ { split(substr($0, 4), field, ","); print label "\t" file ":" field[1] "\t" field[2] }
            /^end_of_record/ { file = "" }
        ' "$path"
    done
}

status=0
table=$(records | awk -F'\t' -v fail_under="$fail_under" '
    {
        key = $1 SUBSEP $2
        if (!(key in seen)) { seen[key] = 1; found[$1]++; if (!($1 in order)) { order[$1] = ++labels; name[labels] = $1 } }
        if ($3 > 0 && !(key in covered)) { covered[key] = 1; hit[$1]++ }
        if (!($2 in all)) { all[$2] = 1; total++ }
        if ($3 > 0 && !($2 in any)) { any[$2] = 1; total_hit++ }
    }
    function pct(h, f) { return f ? 100 * h / f : 100 }
    END {
        print "| Coverage | Lines | Covered | Percent |"
        print "| --- | ---: | ---: | ---: |"
        for (i = 1; i <= labels; i++) printf "| %s | %d | %d | %.2f%% |\n", name[i], found[name[i]], hit[name[i]], pct(hit[name[i]], found[name[i]])
        printf "| Combined | %d | %d | %.2f%% |\n", total, total_hit, pct(total_hit, total)
        if (fail_under != "" && pct(total_hit, total) < fail_under + 0) {
            printf "Combined line coverage %.2f%% is under %s%%\n", pct(total_hit, total), fail_under > "/dev/stderr"
            exit 1
        }
    }') || status=$?
printf '%s\n' "$table"
printf '%s\n' "$table" >>"$summary"
exit "$status"
