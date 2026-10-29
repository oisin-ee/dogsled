#!/usr/bin/env bash
# Deprecate the npm packages that pi-dogsled 0.28 replaces:
# - @oisin-ee/pi-todo: the todo tool ships in @oisin-ee/pi-dogsled.
# - @oisin-ee/oisinpi: install pi-dogsled, pi-prewalk, pi-tiers, and pi-guardrails directly.
#
# Run it only after $release of every package is on npm. Deprecation keeps every
# version installable; it only adds a warning, and `npm deprecate <pkg> ""` undoes it.
#
# Requirements: `npm login` as a package owner, and 2FA. On the npm web page,
# select "skip two-factor authentication for 5 minutes" so both packages go through.
#
# Usage: scripts/npm-retire.sh [--dry-run]
set -euo pipefail

release=0.28.1
run=()
[[ ${1:-} == --dry-run ]] && run=(echo)

npm whoami >/dev/null || { echo "Run npm login first." >&2; exit 1; }

for package in @oisin-ee/pi-dogsled @oisin-ee/pi-prewalk @oisin-ee/pi-tiers @oisin-ee/pi-guardrails @oisin-ee/oisinpi; do
  npm view "$package@$release" version >/dev/null 2>&1 \
    || { echo "$package@$release is not on npm yet. Release first." >&2; exit 1; }
done

"${run[@]}" npm deprecate @oisin-ee/pi-todo \
  "The todo tool is part of @oisin-ee/pi-dogsled since $release. Uninstall @oisin-ee/pi-todo and install @oisin-ee/pi-dogsled."
"${run[@]}" npm deprecate @oisin-ee/oisinpi \
  "Install @oisin-ee/pi-dogsled, @oisin-ee/pi-prewalk, @oisin-ee/pi-tiers, and @oisin-ee/pi-guardrails directly (version $release or later)."

[[ ${#run[@]} -eq 0 ]] || exit 0
for package in @oisin-ee/pi-todo @oisin-ee/oisinpi; do
  printf '%s: %s\n' "$package" "$(npm view "$package" deprecated)"
done
