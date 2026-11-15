#!/usr/bin/env bash
# What a visitor or a box can reach today, against what is merged on dev.
# Every line is read from the service or the host that serves it (GitHub,
# the site, CloudFormation, the running try box), never from a config
# file. "Done" in a session's report means the line here says live.
#
# Usage: scripts/live-state.sh        (make live-state)
# Needs: git, gh (signed in), curl; aws for the relay's lines (skipped
#        without credentials). Reads only; changes nothing.
# Exit: 0 when everything is live, 1 when something on dev is not.
set -uo pipefail
cd "$(dirname "$0")/.."

site="${HELMSMAN_SITE:-https://helmsman.advin.io}"
repo=JimothyJohn/helmsman
relay=JimothyJohn/agent-relay
devices=JimothyJohn/helmsman-devices
# The try box's checkout sits beside the main checkout (also from a worktree).
main="$(cd "$(git rev-parse --git-common-dir)/.." && pwd)"
try="${HELMSMAN_TRY:-$(dirname "$main")/helmsman-try}"
behind=0

row() { printf '%-10s %-22s %s\n' "$1" "$2" "$3"; }
live() { row live "$1" "$2"; }
stale() { row "NOT LIVE" "$1" "$2"; behind=1; }
unknown() { row unknown "$1" "$2"; }

# Commits BASE lacks from HEAD in REPO, or nothing when GitHub will not say.
ahead() { gh api "repos/$1/compare/$2...$3" --jq .ahead_by 2>/dev/null; }

promotion() { # LABEL REPO BASE HEAD
  local n
  n="$(ahead "$2" "$3" "$4")"
  case "$n" in
    "") unknown "$1" "GitHub did not answer for $2 ($3...$4)" ;;
    0) live "$1" "$3 has all of $4" ;;
    *) stale "$1" "$3 is $n commits behind $4" ;;
  esac
}

promotion "helmsman" "$repo" master dev

# The page the site serves, against the one built on dev: each build has
# its own file name.
built() { grep -o 'helmsman-canvas-[0-9a-f]\{16\}' | head -1; }
on_dev="$(git fetch -q origin dev 2>/dev/null; git show origin/dev:docs/playground/index.html 2>/dev/null | built)"
on_site="$(curl -fsS --max-time 20 "$site/playground/" 2>/dev/null | built)"
if [ -z "$on_site" ]; then
  unknown "playground" "$site/playground/ did not answer"
elif [ "$on_site" = "$on_dev" ]; then
  live "playground" "the site serves dev's build (${on_site#helmsman-canvas-})"
else
  stale "playground" "the site serves ${on_site#helmsman-canvas-}, dev holds ${on_dev#helmsman-canvas-}"
fi
# The page on dev is itself a build: older than the canvas it was built from?
page_at="$(git log -1 --format=%ct origin/dev -- docs/playground 2>/dev/null)"
code_at="$(git log -1 --format=%ct origin/dev -- crates/helmsman-canvas crates/helmsman-iec crates/helmsman-diagram crates/helmsman-sim 2>/dev/null)"
if [ -n "$page_at" ] && [ -n "$code_at" ] && [ "$code_at" -gt "$page_at" ]; then
  stale "playground" "dev's page was built before the canvas last changed: scripts/build-playground.sh"
fi

promotion "relay" "$relay" main dev
if command -v aws >/dev/null && aws sts get-caller-identity >/dev/null 2>&1; then
  deployed="$(aws cloudformation describe-stacks --stack-name agent-relay \
    --query 'Stacks[0].LastUpdatedTime' --output text 2>/dev/null)"
  merged="$(gh api "repos/$relay/commits/main" --jq .commit.committer.date 2>/dev/null)"
  if [ -z "$deployed" ] || [ -z "$merged" ]; then
    unknown "relay deploy" "no answer from CloudFormation or GitHub"
  else
    # Both are UTC ISO 8601; the first 19 characters compare as text.
    if [[ "${deployed:0:19}" > "${merged:0:19}" ]]; then
      live "relay deploy" "deployed ${deployed:0:16}Z, after main's tip (${merged:0:16}Z)"
    else
      stale "relay deploy" "deployed ${deployed:0:16}Z, before main's tip (${merged:0:16}Z)"
    fi
  fi
else
  unknown "relay deploy" "no AWS credentials here"
fi

promotion "device library" "$devices" main dev

# The running box says which build it is (X-Helmsman-Build: the commit it
# was built from, `git describe --always --abbrev=9`).
stamp="$(curl -sk -D - -o /dev/null --max-time 5 https://127.0.0.1:8081/api 2>/dev/null \
  | tr -d '\r' | awk 'tolower($1) == "x-helmsman-build:" { print $2 }')"
if [ -z "$stamp" ]; then
  if [ -d "$try" ]; then stale "try box" ":8081 does not answer (launchctl kickstart -k gui/$(id -u)/io.advin.helmsman-try)"
  else unknown "try box" "no box on :8081 and no checkout at $try"; fi
else
  n="$(git rev-list --count "$stamp..origin/dev" 2>/dev/null)"
  case "$n" in
    "") unknown "try box" "running build $stamp, not a commit this checkout knows" ;;
    0) live "try box" "running dev's tip ($stamp)" ;;
    *) stale "try box" "running $stamp, $n commits behind dev ($try/scratch/16-rebuild.sh)" ;;
  esac
fi

exit "$behind"
