#!/bin/sh
# Find the release that main still owes, and settle every merged release PR that is not it.
#
# A merged release PR keeps the `autorelease: pending` label until its tag exists. While
# any PR has that label, release-please refuses to open the next release PR. So every run
# looks at every such PR, not only the one this push merged:
#
# - Its tag exists: an earlier run published it. Relabel it `autorelease: tagged`.
# - The manifest on main no longer holds its version: its release commit was reverted, so
#   it will never be published. Relabel it `autorelease: superseded`.
# - Otherwise it is the outstanding release. The gate validates only the commit this run
#   checked out, so the tag goes only on that commit. When this run is for the release
#   commit itself (its push, or a dispatch retry before main moved on), print its tag and
#   commit. When main has moved past it, nothing can validate the release commit any more:
#   fail, and name the revert that supersedes it.
#
# Output: `release-pr=`, `tag=` and `sha=` lines for $GITHUB_OUTPUT, or nothing when no
# release is outstanding. With DRY_RUN=1 the script changes no labels.
set -eu

repo=${GITHUB_REPOSITORY:?}
head_sha=$(git rev-parse HEAD)
head_version=$(jq -er '.["."]' .release-please-manifest.json)

relabel() {
	if [ "${DRY_RUN:-0}" = 1 ]; then
		echo "dry run: would relabel #$1 as $2" >&2
		return
	fi
	gh label create "$2" --repo "$repo" --force >/dev/null
	gh pr edit "$1" --repo "$repo" --remove-label "autorelease: pending" --add-label "$2" >/dev/null
}

# Read the list first: a failed lookup must fail the run, not look like no release.
pending=$(gh pr list --repo "$repo" --state merged --base main --label "autorelease: pending" \
	--limit 100 --json number,headRefName,mergeCommit,mergedAt \
	--jq 'sort_by(.mergedAt) | .[] | select(.headRefName | startswith("release-please--")) | "\(.number) \(.mergeCommit.oid)"')
outstanding=""
printf '%s\n' "$pending" | grep . |
	{
		while read -r pr sha; do
			version=$(git show "$sha:.release-please-manifest.json" | jq -er '.["."]')
			tag="v$version"
			if [ -n "$(git ls-remote --tags origin "refs/tags/$tag")" ]; then
				echo "#$pr: $tag exists" >&2
				relabel "$pr" "autorelease: tagged"
			elif [ "$version" != "$head_version" ]; then
				echo "#$pr: main's manifest holds $head_version, not $version; superseded" >&2
				relabel "$pr" "autorelease: superseded"
			elif [ -n "$outstanding" ]; then
				echo "#$pr and #$outstanding both claim $tag" >&2
				exit 1
			elif [ "$sha" != "$head_sha" ]; then
				echo "#$pr: $tag never passed its gate at $sha, and main has moved on to $head_sha." >&2
				echo "Supersede it: revert $sha on main with a chore(release): subject." >&2
				exit 1
			else
				outstanding=$pr
				echo "#$pr: $tag is outstanding at $sha" >&2
				printf 'release-pr=%s\ntag=%s\nsha=%s\n' "$pr" "$tag" "$sha"
			fi
		done
	}
