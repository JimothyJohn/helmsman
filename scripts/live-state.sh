#!/usr/bin/env bash
# What a visitor or a box can reach today, against what is merged on dev.
# Every line is read from the service or the host that serves it (GitHub,
# the site, CloudFormation, the running try box), never from a config
# file. "Done" in a session's report means the line here says live.
#
# Usage: scripts/live-state.sh        (make live-state)
#        scripts/live-state.sh --at helmsman=SHA --at agent-relay=SHA ...
#          holds a repository to that commit instead of its dev tip
#          (scripts/ship.sh: live is what it shipped, not what landed on
#          dev since). Repositories: helmsman, agent-relay, helmsman-devices.
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
at_helmsman=dev at_relay=dev at_devices=dev
while [ $# -gt 0 ]; do
  case "$1" in
    --at)
      [ $# -ge 2 ] || { echo "live-state.sh: --at needs REPO=SHA" >&2; exit 2; }
      v=${2#*=}
      printf '%s' "$v" | grep -Eq '^[0-9a-f]{7,40}$' || { echo "live-state.sh: --at $2: not a commit" >&2; exit 2; }
      case "${2%%=*}" in
        helmsman) at_helmsman=$v ;;
        agent-relay) at_relay=$v ;;
        helmsman-devices) at_devices=$v ;;
        *) echo "live-state.sh: --at $2: unknown repository" >&2; exit 2 ;;
      esac
      shift 2 ;;
    *) echo "live-state.sh: unknown argument $1" >&2; exit 2 ;;
  esac
done
# A commit is named short in the rows; dev is named as dev.
name() { [ "$1" = dev ] && echo dev || echo "${1:0:12}"; }

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
    0) live "$1" "$3 has all of $(name "$4")" ;;
    *) stale "$1" "$3 is $n commits behind $(name "$4")" ;;
  esac
}

promotion "helmsman" "$repo" master "$at_helmsman"

# The page the site serves, against the one built on dev: each build has
# its own file name.
built() { grep -o 'helmsman-canvas-[0-9a-f]\{16\}' | head -1; }
git fetch -q origin dev 2>/dev/null
page_ref=origin/dev; [ "$at_helmsman" = dev ] || page_ref=$at_helmsman
on_dev="$(git show "$page_ref:docs/playground/index.html" 2>/dev/null | built)"
on_site="$(curl -fsS --max-time 20 "$site/playground/" 2>/dev/null | built)"
if [ -z "$on_site" ]; then
  unknown "playground" "$site/playground/ did not answer"
elif [ "$on_site" = "$on_dev" ]; then
  live "playground" "the site serves $(name "$at_helmsman")'s build (${on_site#helmsman-canvas-})"
else
  stale "playground" "the site serves ${on_site#helmsman-canvas-}, $(name "$at_helmsman") holds ${on_dev#helmsman-canvas-}"
fi
# The page on dev is itself a build: has the canvas changed since the commit
# that last wrote it? (A diff, not commit dates: a branch merged late keeps
# its old dates.) scripts/ship.sh asks the same question before it ships.
page_commit="$(git log -1 --format=%H "$page_ref" -- docs/playground 2>/dev/null)"
if [ -n "$page_commit" ] && ! git diff --quiet "$page_commit" "$page_ref" -- \
    crates/helmsman-canvas crates/helmsman-iec crates/helmsman-diagram crates/helmsman-sim 2>/dev/null; then
  stale "playground" "$(name "$at_helmsman")'s page was built before the canvas last changed: scripts/build-playground.sh"
fi

promotion "relay" "$relay" main "$at_relay"
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

promotion "device library" "$devices" main "$at_devices"

# The try box, when it is up, says which build it is (X-Helmsman-Build: the commit it
# was built from, `git describe --always --abbrev=9`).
stamp="$(curl -sk -D - -o /dev/null --max-time 5 https://127.0.0.1:8081/api 2>/dev/null \
  | tr -d '\r' | awk 'tolower($1) == "x-helmsman-build:" { print $2 }')"
if [ -z "$stamp" ]; then
  # Off is its normal state (CLAUDE.md, the try box): nothing to be behind.
  row off "try box" "not running; start it when a task needs a box (CLAUDE.md, Checkouts and services)"
else
  n="$(git rev-list --count "$stamp..origin/dev" 2>/dev/null)"
  case "$n" in
    "") unknown "try box" "running build $stamp, not a commit this checkout knows" ;;
    0) live "try box" "running dev's tip ($stamp)" ;;
    *) stale "try box" "running $stamp, $n commits behind dev ($try/scratch/16-rebuild.sh)" ;;
  esac
fi

exit "$behind"
