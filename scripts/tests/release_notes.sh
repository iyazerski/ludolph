#!/usr/bin/env bash
set -euo pipefail

script="$(cd "$(dirname "$0")/.." && pwd)/release_notes.sh"
work=$(mktemp -d /tmp/ludolph-release-notes.XXXXXX)
trap 'rm -rf "$work"' EXIT

git init -q "$work/repo"
cd "$work/repo"
git config user.name Fixture
git config user.email fixture@example.com
git config commit.gpgsign false
git config core.hooksPath /dev/null
git commit -q --allow-empty -m Base
base=$(git rev-parse HEAD)
git commit -q --allow-empty -m 'Fix a direct change'
direct=$(git rev-parse HEAD)
git commit -q --allow-empty -m 'A squash title that should not appear'
FIXTURE_MERGE=$(git rev-parse HEAD)
export FIXTURE_MERGE
git commit -q --allow-empty -m 'A later change outside the release'

# Stand in for the GitHub transport, leaving Git, JSON parsing and rendering real.
gh() {
  if [[ "${FAIL_API:-}" == all || "${FAIL_API:-}" == "$3" ]]; then
    echo 'GitHub unavailable' >&2
    return 1
  fi
  [[ "$1" == api && "$2" == --paginate ]]
  case "$3" in
    "repos/owner/repo/commits/$FIXTURE_MERGE/pulls?per_page=100")
      printf '[{"number":99,"merged_at":null,"merge_commit_sha":"%s"},{"number":60,"merged_at":"2026-11-14","merge_commit_sha":"%s"}]\n' "$FIXTURE_MERGE" "$FIXTURE_MERGE"
      ;;
    'repos/owner/repo/commits/'*'/pulls?per_page=100') printf '[]\n' ;;
    graphql)
      printf '%s\n' \
        '{"data":{"repository":{"pullRequest":{"commits":{"nodes":[{"commit":{"messageHeadline":"Keep skill mentions inline"}}],"pageInfo":{"hasNextPage":true,"endCursor":"page-2"}}}}}}' \
        '{"data":{"repository":{"pullRequest":{"commits":{"nodes":[{"commit":{"messageHeadline":"Use the same info icon"}}],"pageInfo":{"hasNextPage":false,"endCursor":"end"}}}}}}'
      ;;
    *) echo "Unexpected API endpoint: $3" >&2; return 1 ;;
  esac
}
export -f gh
export GH_REPO=owner/repo

actual=$(bash "$script" "$base..$FIXTURE_MERGE")
expected='- Keep skill mentions inline ([#60](https://github.com/owner/repo/pull/60))
- Use the same info icon ([#60](https://github.com/owner/repo/pull/60))
- Fix a direct change'
if [[ "$actual" != "$expected" ]]; then
  printf 'Unexpected release notes:\n%s\n' "$actual" >&2
  exit 1
fi

[[ $(bash "$script" "$base..$direct") == '- Fix a direct change' ]]
[[ -z $(bash "$script" "$base..$base") ]]

for failure in all graphql; do
  if FAIL_API="$failure" bash "$script" "$base..$FIXTURE_MERGE" > "$work/notes" 2> "$work/error"; then
    echo 'An API failure must stop release publication' >&2
    exit 1
  fi
done
printf 'Release-note checks passed\n'
