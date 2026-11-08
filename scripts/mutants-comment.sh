#!/usr/bin/env bash
# Keep one comment on the pull request listing the mutants no test caught
# (what scripts/mutants.sh found), and delete it once every one is caught.
# Usage: scripts/mutants-comment.sh RESULTS
# Needs GH_TOKEN, GITHUB_REPOSITORY, PR and RUN; FORK=true only summarizes.
# MUTANTS_COMMENT_LIMIT overrides the comment's size in bytes (for tests).
set -euo pipefail
# Bytes, not characters: GitHub limits a comment to 65,536 characters, and a
# step summary to 1 MiB.
export LC_ALL=C
results=${1:?Usage: scripts/mutants-comment.sh RESULTS}
marker='<!-- mutants -->'
limit=${MUTANTS_COMMENT_LIMIT:-60000}
summary_limit=1000000
lived=$(cat "$results/missed.txt" "$results/timeout.txt" 2>/dev/null | sort -u || true)
# No results (nothing to mutate) counts as every mutant caught.
caught=$(grep -c . "$results/caught.txt" 2>/dev/null || true)
caught=${caught:-0}
count=$(grep -c . <<<"$lived" || true)
total=$((caught + count))
# The comment for a list of changes. A list over `budget` bytes is cut after
# a whole line, so the code fence stays closed, and says how many it shows.
render() {
    local list=$1 budget=$2 shown=$count note=''
    if ((${#list} > budget)); then
        list=$(printf '%s\n' "$list" | head -c "$budget" | sed '$d')
        shown=$(grep -c . <<<"$list" || true)
        note="
Showing $shown of $count. The rest is in the [full report]($RUN)."
    fi
    local fence="\`\`\`"
    printf '%s\n**Mutation testing:** %s of %s changes caught by a test.\n\n%s\n%s\n%s\n%s\nEach one is a change to the code no test noticed. Not a blocker: real gaps get their own PR. [Full report](%s)' \
        "$marker" "$caught" "$total" "$fence" "$list" "$fence" "$note" "$RUN"
}
if [[ -n $lived ]]; then
    render "$lived" "$summary_limit" | head -c "$summary_limit" >>"${GITHUB_STEP_SUMMARY:-/dev/null}"
    printf '\n' >>"${GITHUB_STEP_SUMMARY:-/dev/null}"
fi
[[ ${FORK:-false} != true ]] || exit 0
# The room the text around the list takes, with a margin for the note.
overhead=$(render '' 0 | wc -c)
body=$(render "$lived" $((limit - overhead - 256)))
old=$(gh api --paginate "repos/$GITHUB_REPOSITORY/issues/$PR/comments" \
    --jq ".[] | select(.user.login == \"github-actions[bot]\" and (.body | startswith(\"$marker\"))) | .id" | head -n 1)
if [[ -z $lived ]]; then
    if [[ -n $old ]]; then gh api -X DELETE "repos/$GITHUB_REPOSITORY/issues/comments/$old"; fi
elif [[ -n $old ]]; then
    gh api -X PATCH "repos/$GITHUB_REPOSITORY/issues/comments/$old" -f body="$body" >/dev/null
else
    gh api "repos/$GITHUB_REPOSITORY/issues/$PR/comments" -f body="$body" >/dev/null
fi
