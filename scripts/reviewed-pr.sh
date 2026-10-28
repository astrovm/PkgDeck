#!/usr/bin/env bash
# Skip validation only when a successful PR run recorded the exact tested tree.
# Missing, expired or mismatched proof requires validation to run again.
# Usage: GH_TOKEN=... scripts/reviewed-pr.sh <commit sha> [workflow filename]
set -euo pipefail
sha=${1:?Usage: scripts/reviewed-pr.sh <commit sha> [workflow filename]}
workflow=${2:-ci.yml}
repo=${GITHUB_REPOSITORY:?GITHUB_REPOSITORY is required}

pr=$("$(dirname "${BASH_SOURCE[0]}")/merged-pr.sh" "$sha") || exit 1
IFS=$'\t' read -r pr_number head_sha head_repo <<<"$pr"
tree=$(gh api "repos/$repo/git/commits/$sha" --jq '.tree.sha')

runs=$(gh api --method GET "repos/$repo/actions/workflows/$workflow/runs" \
    -f event=pull_request -f "head_sha=$head_sha" -f per_page=100 |
    jq -r --arg sha "$head_sha" --arg repo "$head_repo" --arg path ".github/workflows/$workflow" \
        '.workflow_runs[] | select(.event == "pull_request" and .head_sha == $sha and .head_repository.full_name == $repo and .status == "completed" and .conclusion == "success" and .path == $path) | .id')
proof=$(mktemp -d)
trap 'rm -rf "$proof"' EXIT
while IFS= read -r run_id; do
    [[ "$run_id" =~ ^[0-9]+$ ]] || continue
    mkdir "$proof/$run_id"
    if ! gh run download "$run_id" --repo "$repo" --name tested-source-tree --dir "$proof/$run_id"; then
        continue
    fi
    if [[ -f "$proof/$run_id/tree" ]] && [[ "$(cat "$proof/$run_id/tree")" == "$tree" ]]; then
        echo "$sha matches the tested tree from PR #$pr_number, run $run_id."
        exit 0
    fi
done <<<"$runs"
echo "::notice::PR #$pr_number has no successful run proving this exact tree."
exit 1
