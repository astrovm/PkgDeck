#!/usr/bin/env bash
# Find the pull request a commit on main came from, and prove that pull
# request's CI already tested exactly this source.
#
# The commit must be the PR's squash merge, its tree must equal the PR head's
# tree (so main had nothing the PR never saw), and CI must have passed on that
# head. Prints the PR number and the passing run; fails with an ::error::
# otherwise.
#
# Usage: GH_TOKEN=... scripts/reviewed-pr.sh <commit sha>
set -euo pipefail
sha=${1:?Usage: scripts/reviewed-pr.sh <commit sha>}
repo=${GITHUB_REPOSITORY:?GITHUB_REPOSITORY is required}

pr=$(gh api "repos/$repo/commits/$sha/pulls" |
    jq -r --arg sha "$sha" \
        '[.[] | select(.merged_at != null and .base.ref == "main" and .merge_commit_sha == $sha)] | first | if . == null then empty else [.number, .head.sha, .head.repo.full_name] | @tsv end')
if [[ -z "$pr" ]]; then
    echo "::error::$sha is not a pull request merge commit on main."
    exit 1
fi
IFS=$'\t' read -r pr_number head_sha head_repo <<<"$pr"
test -n "$head_sha"
test -n "$head_repo"

head_tree=$(gh api "repos/$repo/git/commits/$head_sha" --jq '.tree.sha')
tree=$(gh api "repos/$repo/git/commits/$sha" --jq '.tree.sha')
if [[ "$head_tree" != "$tree" ]]; then
    echo "::error::$sha differs from the head of PR #$pr_number."
    exit 1
fi

run=$(gh api --method GET "repos/$repo/actions/workflows/ci.yml/runs" \
    -f event=pull_request -f "head_sha=$head_sha" -f per_page=100 |
    jq -r --arg sha "$head_sha" --arg repo "$head_repo" \
        '[.workflow_runs[] | select(.event == "pull_request" and .head_sha == $sha and .head_repository.full_name == $repo and .status == "completed" and .conclusion == "success" and .path == ".github/workflows/ci.yml")] | first | .html_url // empty')
if [[ -z "$run" ]]; then
    echo "::error::PR #$pr_number has no successful CI run for its merged head commit."
    exit 1
fi
echo "$sha matches PR #$pr_number and successful CI run $run"
