#!/usr/bin/env bash
# Require PRs into `dev` to be up to date with dev before they merge
# (branch protection's "strict" status checks), keeping the required checks
# as they are. Pairs with .github/workflows/update-branches.yml, which
# brings auto-merge PRs up to date when dev moves. TODO.md dev-merge-safety-2.
#
# Run it only after the UPDATE_BRANCH_TOKEN secret exists, or strict PRs
# wait behind dev until someone updates them by hand.
# Needs: gh, logged in as the repo owner. Safe to re-run.
# Undo: same call with strict=false.
set -euo pipefail

repo="${1:-JimothyJohn/helmsman}"
gh secret list -R "$repo" | grep -q '^UPDATE_BRANCH_TOKEN' \
    || { echo "set the UPDATE_BRANCH_TOKEN secret first (see .github/workflows/update-branches.yml)" >&2; exit 1; }

now="$(gh api "repos/$repo/branches/dev/protection/required_status_checks" --jq .strict)"
if [ "$now" = true ]; then
    echo "dev already requires up-to-date branches"
    exit 0
fi
gh api -X PATCH "repos/$repo/branches/dev/protection/required_status_checks" -F strict=true >/dev/null
[ "$(gh api "repos/$repo/branches/dev/protection/required_status_checks" --jq .strict)" = true ] \
    || { echo "strict did not stick" >&2; exit 1; }
echo "dev now requires up-to-date branches; checks: $(gh api "repos/$repo/branches/dev/protection/required_status_checks" --jq '.contexts | join(", ")')"
