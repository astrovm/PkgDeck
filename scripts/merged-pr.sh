#!/usr/bin/env bash
# The pull request that merged a commit into main, directly or as part of a
# stack: merged into the branch of a pull request below it that reached main,
# as a stack merges bottom first. Prints its number, head commit and head
# repository, tab-separated, or fails when no pull request did.
# Usage: GH_TOKEN=... scripts/merged-pr.sh <commit sha>
set -euo pipefail
sha=${1:?Usage: scripts/merged-pr.sh <commit sha>}
repo=${GITHUB_REPOSITORY:?GITHUB_REPOSITORY is required}
pr=$(gh api "repos/$repo/commits/$sha/pulls" |
    jq -r --arg sha "$sha" \
        '[.[] | select(.merged_at != null and .merge_commit_sha == $sha)] | first | if . == null then empty else [.number, .head.sha, .head.repo.full_name, .base.ref] | @tsv end')
if [[ -z "$pr" ]]; then
    echo "::notice::$sha has no matching merged pull request." >&2
    exit 1
fi
IFS=$'\t' read -r number head_sha head_repo base <<<"$pr"
test -n "$head_sha"
test -n "$head_repo"
# Each branch below main must be the head of a merged pull request, down to
# one whose base is main. A stack is never this deep; a loop stops here.
for _ in {1..20}; do
    if [[ $base == main ]]; then
        printf '%s\t%s\t%s\n' "$number" "$head_sha" "$head_repo"
        exit 0
    fi
    below=$(gh api --method GET "repos/$repo/pulls" -f state=closed -f "head=${repo%%/*}:$base" -f per_page=100 |
        jq -r '[.[] | select(.merged_at != null)] | first | .base.ref // empty')
    if [[ -z "$below" ]]; then
        echo "::notice::PR #$number was merged into $base, which no merged pull request brought to main." >&2
        exit 1
    fi
    base=$below
done
echo "::notice::PR #$number sits on a stack too deep to follow." >&2
exit 1
