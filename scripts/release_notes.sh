#!/usr/bin/env bash
set -euo pipefail

# Print commit subjects for a release range, expanding squash merges through GitHub.
: "${GH_REPO:?GH_REPO must name the owner/repository}"
range="${1:?usage: release_notes.sh REVISION_RANGE}"

git log --first-parent --format='%H' "$range" | while IFS= read -r sha; do
  pull=$(gh api --paginate "repos/$GH_REPO/commits/$sha/pulls?per_page=100" |
    jq -r --arg sha "$sha" '.[] | select(.merged_at != null and .merge_commit_sha == $sha) | .number')
  if [[ -z "$pull" ]]; then
    git show -s --format='- %s' "$sha"
    continue
  fi
  # GitHub resolves the GraphQL variables, not the shell.
  # shellcheck disable=SC2016
  gh api --paginate graphql \
    -F owner="${GH_REPO%/*}" -F name="${GH_REPO#*/}" -F number="$pull" \
    -f query='
      query($owner: String!, $name: String!, $number: Int!, $endCursor: String) {
        repository(owner: $owner, name: $name) {
          pullRequest(number: $number) {
            commits(first: 100, after: $endCursor) {
              nodes { commit { messageHeadline } }
              pageInfo { hasNextPage endCursor }
            }
          }
        }
      }' |
    jq -r --arg pull "$pull" --arg repo "$GH_REPO" \
      '.data.repository.pullRequest.commits.nodes[] |
        "- \(.commit.messageHeadline) ([#\($pull)](https://github.com/\($repo)/pull/\($pull)))"'
done
