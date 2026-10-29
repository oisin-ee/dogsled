#!/usr/bin/env bash
# Point npm trusted publishing for every package of this repository at
# oisin-ee/dogsled and release.yml, which calls publish-npm.yml.
#
# Requirements: npm 11.15 or later, `npm login` as a package owner, and 2FA.
# The first change asks for a one-time password. On the npm web page, select
# "skip two-factor authentication for 5 minutes" so the other packages go through.
#
# Usage: scripts/npm-trust.sh [--dry-run]
set -euo pipefail

repo=oisin-ee/dogsled
file=release.yml
dry_run=()
[[ ${1:-} == --dry-run ]] && dry_run=(--dry-run)

root=$(git -C "$(dirname "$0")" rev-parse --show-toplevel)
mapfile -t packages < <(
  for manifest in "$root/package.json" "$root"/packages/*/package.json; do
    node -p 'require(process.argv[1]).name' "$manifest"
  done
)

npm whoami >/dev/null || { echo "Run npm login first." >&2; exit 1; }

# Every trust command, `list` included, needs 2FA. A captured call cannot show the
# prompt, so authenticate once on the terminal first.
echo "Authenticating with npm. Select 'skip two-factor authentication for 5 minutes'."
npm trust list "${packages[0]}" >/dev/null

# `npm trust list --json` prints one object or an array; normalize to lines of
# "<id> <repository> <file>". A failed list stops the script: it must never read as
# "no configuration".
configs() {
  local json
  json=$(npm trust list "$1" --json) || { echo "$1: npm trust list failed" >&2; exit 1; }
  jq -r 'if type == "array" then .[] else . end
         | select(.id != null)
         | "\(.id) \(.repository // "-") \(.file // "-")"' <<<"$json"
}

for package in "${packages[@]}"; do
  current=$(configs "$package")
  if grep -qx "[^ ]* $repo $file" <<<"$current"; then
    echo "$package: already trusts $repo $file"
    continue
  fi
  # The registry keeps one configuration per package, so revoke the old one first.
  while read -r id old_repo old_file; do
    [[ -n $id ]] || continue
    echo "$package: revoke $id ($old_repo $old_file)"
    [[ ${#dry_run[@]} -eq 0 ]] && npm trust revoke "$package" --id "$id"
  done <<<"$current"
  echo "$package: trust $repo $file"
  npm trust github "$package" --repo "$repo" --file "$file" --allow-publish --yes "${dry_run[@]}"
  sleep 2
done

echo
for package in "${packages[@]}"; do
  printf '%s: %s\n' "$package" "$(configs "$package" | tr '\n' ';')"
done
