#!/usr/bin/env bash
# make ship: promote the release train and show, feature by feature, that
# live does what dev does (docs/PARITY.md). Nick runs it; nothing else
# promotes (the dev -> default merge stays his, by this command).
#
#   scripts/ship.sh             check the candidate, ask, promote, deploy,
#                               wait for live, check live, report
#   scripts/ship.sh --dry-run   the candidate half only: nothing promoted
#   scripts/ship.sh --yes       no question before promoting
#   scripts/ship.sh --live-only the live half only (after a ship whose
#                               live check you want again)
#   scripts/ship.sh --report    the report again from target/ship/ (runs
#                               nothing, opens no pull request)
#
# Steps:
#   0. playground: when the canvas has changed since dev's committed page
#      was built, the page is rebuilt at the dev tip
#      (scripts/build-playground.sh, with the relay stack's DemoUrl and
#      DemoDesignUrl, or HELMSMAN_FREE_CHAT_URL / HELMSMAN_DESIGN_URL when
#      set) and merged into dev by a pull request before anything is
#      checked. Without this the site kept a page six days old through a
#      ship (2026-11-21). --dry-run only says the page is stale.
#   1. candidate: scripts/compat.sh --candidate (the four dev tips against
#      each other's contracts, and their CI), then `helmsman-compat parity`
#      on the box compat built at dev, and scripts/parity-tabs.py on dev's
#      committed playground (what master will serve).
#   2. promote: each repository whose candidate commit (compat.sh's
#      target/compat/train.txt) is ahead of its default branch gets a
#      ship/<stamp> branch at that commit and a pull request from it into
#      the default branch, merged (a merge commit). What lands on dev after
#      the candidate was checked is neither shipped nor in the way. agent-relay is then deployed from a
#      throwaway worktree at main (`make deploy`, which keeps the stack's
#      settings), so the relay that is live is the one that was checked.
#   3. live: scripts/live-state.sh --at each candidate commit until every
#      row but the try box says live (the site's Pages build takes minutes), then parity on a box
#      built at the new master held to dev's docs/FEATURES.md, and the
#      tabs on the public site.
#   4. report: target/ship/report.md, every feature with its candidate and
#      live outcome. A feature that passed on the candidate and fails live
#      is a regression: the report names it and a draft pull request
#      reverting helmsman's promotion is opened on master for Nick to
#      merge or close. Nothing is ever force-pushed or reset.
#
# Needs: what scripts/compat.sh needs (cargo, git, gh signed in, the
# sibling checkouts), Playwright's Chromium for python3, trunk and the aws
# CLI for a playground rebuild, and for the relay
# deploy what ../agent-relay/scripts/deploy.sh needs (AWS credentials,
# SAM, cargo-lambda). Run it from a real terminal: a box that reads the
# LAN is not involved, but the relay deploy and gh may ask for things.
# Exit: 0 shipped and live matches, 1 a check failed or live regressed,
# 2 something could not run, 3 Nick said no.
set -uo pipefail
cd "$(dirname "$0")/.."
ROOT=$(pwd)
MAIN=$(cd "$(git rev-parse --git-common-dir)/.." && pwd)
REPOS=${COMPAT_REPOS:-$(dirname "$MAIN")}
OUT=$ROOT/target/ship
SITE=${HELMSMAN_SITE:-https://helmsman.advin.io}
DRY=0 YES=0 LIVE_ONLY=0 REPORT_ONLY=0
for a in "$@"; do
  case "$a" in
    --dry-run) DRY=1 ;;
    --yes) YES=1 ;;
    --live-only) LIVE_ONLY=1 ;;
    --report) REPORT_ONLY=1 ;;
    -h|--help) sed -n '2,40p' "$0"; exit 0 ;;
    *) echo "ship.sh: unknown argument $a (--help)" >&2; exit 2 ;;
  esac
done
mkdir -p "$OUT"
say() { printf '\n== %s\n' "$*"; }
die() { echo "ship.sh: $*" >&2; exit 2; }

default_of() {
  case "$1" in
    helmsman|helmsman-hardware) echo master ;;
    *) echo main ;;
  esac
}
src_of() { [ "$1" = helmsman ] && echo "$MAIN" || echo "$REPOS/$1"; }
slug() { git -C "$(src_of "$1")" remote get-url origin | sed -E 's#(git@github.com:|https://github.com/)##; s#\.git$##'; }
REPOS_ALL="helmsman agent-relay helmsman-devices helmsman-hardware"
TRAIN=$ROOT/target/compat/train.txt
# candidate REPO -> the commit compat.sh checked for it (empty without a run)
candidate() { awk -v r="$1" '$1 == r { print $2 }' "$TRAIN" 2>/dev/null; }

# page_stale REF -> 0 when the canvas changed after the commit that last
# wrote REF's docs/playground (the question scripts/live-state.sh asks).
page_stale() {
  local page; page=$(git -C "$MAIN" log -1 --format=%H "$1" -- docs/playground)
  [ -n "$page" ] && ! git -C "$MAIN" diff --quiet "$page" "$1" -- \
    crates/helmsman-canvas crates/helmsman-iec crates/helmsman-diagram crates/helmsman-sim
}
# stack_out KEY -> the relay stack's output KEY, or nothing
stack_out() {
  local v; v=$(aws cloudformation describe-stacks --stack-name agent-relay \
    --query "Stacks[0].Outputs[?OutputKey=='$1'].OutputValue | [0]" --output text 2>/dev/null)
  [ "$v" = None ] || printf '%s' "$v"
}
# wait_merged URL MINUTES -> 0 once the pull request merged; 1 closed or timed out
wait_merged() {
  local st
  for _ in $(seq 1 $(($2 * 4))); do
    st=$(gh pr view "$1" --json state --jq .state 2>/dev/null)
    [ "$st" = MERGED ] && return 0
    [ "$st" = CLOSED ] && return 1
    sleep 15
  done
  return 1
}

if [ "$REPORT_ONLY" = 1 ]; then
  H_DEV=$(git -C "$MAIN" rev-parse origin/dev); H_LIVE=$(git -C "$MAIN" rev-parse origin/master)
  DEV_DIR=$ROOT/target/compat/src/helmsman@${H_DEV:0:12}
  FEATURES=$DEV_DIR/docs/FEATURES.md
  [ -f "$FEATURES" ] || die "--report needs a run's checkout of dev at $DEV_DIR"
else
# ---- 1. candidate ----------------------------------------------------------

python3 -I -c 'import playwright' 2>/dev/null || die "needs Playwright for python3 (pip install playwright; python3 -m playwright install chromium)"
gh auth status >/dev/null 2>&1 || die "gh is not signed in"

# ---- 0. the playground -----------------------------------------------------

if [ "$LIVE_ONLY" = 0 ]; then
  git -C "$MAIN" fetch -q origin dev || die "fetching helmsman failed"
  if ! page_stale origin/dev; then
    :
  elif [ "$DRY" = 1 ]; then
    say "playground: dev's page is older than its canvas; a real ship rebuilds it first"
  else
    say "playground: dev's page is older than its canvas; rebuilding it at the dev tip"
    command -v trunk >/dev/null || die "the playground rebuild needs trunk (cargo install trunk --locked)"
    free=${HELMSMAN_FREE_CHAT_URL-$(stack_out DemoUrl)}
    design=${HELMSMAN_DESIGN_URL-$(stack_out DemoDesignUrl)}
    [ -n "$free" ] || die "no free chat address (the relay stack's DemoUrl, or HELMSMAN_FREE_CHAT_URL): the page would ask visitors for a key"
    br=site/playground-$(date -u +%Y%m%d%H%M); wt=$MAIN/.claude/worktrees/$(echo "$br" | tr '/' '-')
    git -C "$MAIN" worktree add -q -b "$br" "$wt" origin/dev || die "a worktree for $br failed"
    echo "  build at $(git -C "$wt" rev-parse --short=12 HEAD) (free chat ${free%/}${design:+, design ${design%/}}; $OUT/playground.log)"
    (cd "$wt" && HELMSMAN_FREE_CHAT_URL=$free HELMSMAN_DESIGN_URL=$design scripts/build-playground.sh) >"$OUT/playground.log" 2>&1 ||
      { tail -20 "$OUT/playground.log" >&2; die "the playground build failed ($OUT/playground.log); worktree left at $wt"; }
    git -C "$wt" add docs/playground
    if git -C "$wt" diff --cached --quiet; then
      echo "  the build came out the same as dev's page"
    else
      git -C "$wt" commit -q -m "site: rebuild the playground at the dev tip (make ship)" &&
        git -C "$wt" push -q -u origin "$br" || die "pushing $br failed; worktree left at $wt"
      url=$(gh pr create --repo "$(slug helmsman)" --base dev --head "$br" \
        --title "site: rebuild the playground at the dev tip" \
        --body "make ship: the canvas changed after dev's committed page was built, so master would serve an old page. Built by scripts/build-playground.sh; auto-merge takes it in once CI is green." 2>&1 | tail -1)
      case "$url" in https://*) echo "  $url" ;; *) die "opening the playground pull request failed: $url" ;; esac
      wait_merged "$url" 40 || die "$url did not merge in 40 min; nothing promoted"
    fi
    git -C "$MAIN" worktree remove "$wt" && git -C "$MAIN" branch -q -D "$br"
    git -C "$MAIN" fetch -q origin dev
    page_stale origin/dev && die "dev's page is stale again: the canvas changed while the rebuild merged; run make ship again"
  fi
fi

if [ "$LIVE_ONLY" = 0 ]; then
  say "candidate: contracts and CI (scripts/compat.sh --candidate)"
  scripts/compat.sh --candidate
  st=$?
  [ "$st" = 0 ] || { echo "ship.sh: the candidate's contracts or CI are not green (target/compat/report.md); nothing promoted" >&2; exit "$st"; }
fi

# The checkouts and the box compat made at the dev tip: the commits it
# checked, which a --live-only run takes from the last ship's train.
H_DEV=$(candidate helmsman)
[ -n "$H_DEV" ] || { [ "$LIVE_ONLY" = 1 ] && H_DEV=$(git -C "$MAIN" rev-parse origin/dev); }
[ -n "$H_DEV" ] || die "compat.sh left no train ($TRAIN)"
DEV_DIR=$ROOT/target/compat/src/helmsman@${H_DEV:0:12}
[ -d "$DEV_DIR" ] || { mkdir -p "$ROOT/target/compat/src"; git clone -q --shared --no-checkout "$MAIN" "$DEV_DIR" && git -C "$DEV_DIR" checkout -q --detach "$H_DEV"; } || die "no checkout of dev at $DEV_DIR"
FEATURES=$DEV_DIR/docs/FEATURES.md
[ -f "$FEATURES" ] || die "dev ($H_DEV) has no docs/FEATURES.md"
(cargo build -q --locked -p helmsman-compat) || die "building helmsman-compat failed"
CHECK=$ROOT/target/debug/helmsman-compat

# build_box SHA LABEL -> the box binary at that commit (as compat.sh builds it)
build_box() {
  local dir=$ROOT/target/compat/src/helmsman@${1:0:12} target=$ROOT/target/compat/build/helmsman-$2
  [ -d "$dir/.git" ] || { git clone -q --shared --no-checkout "$MAIN" "$dir" && git -C "$dir" checkout -q --detach "$1"; } || return 1
  echo "  build helmsman-plc at ${1:0:12} ($2)" >&2
  (cd "$dir" && CARGO_TARGET_DIR="$target" cargo build -q --locked -p helmsman-plc >"$OUT/build-$2.log" 2>&1) || { tail -20 "$OUT/build-$2.log" >&2; return 1; }
  echo "$target/debug/helmsman-plc"
}

# The relays. The candidate's agent journeys run on the stub model behind
# the relay compat.sh built at agent-relay's dev tip (never a stale local
# build); live's on the deployed relay, found as compat.sh --live finds it.
RELAY_DEV=$ROOT/target/compat/build/agent-relay-dev/debug/agent-relay-local
LIVE_URL=${COMPAT_LIVE_URL:-$(grep -oE 'https://[a-z0-9.-]+\.on\.aws' "$HOME/github/helmsman-try/scratch/run-box.sh" 2>/dev/null | head -1)}
LIVE_TOKEN=${COMPAT_LIVE_TOKEN:-$HOME/github/helmsman-try/scratch/relay-token}

# parity LABEL BOX AGENT-ARGS... -> target/ship/LABEL.json and .txt; its exit status
parity() {
  local label=$1 box=$2; shift 2
  "$CHECK" parity --box "$box" --helmsman "$DEV_DIR" --manifest "$FEATURES" "$@" \
    --work "$OUT/work-$label" --json "$OUT/$label.json" >"$OUT/$label.txt" 2>&1
  local st=$?
  cat "$OUT/$label.txt"
  return "$st"
}

# tabs LABEL ARGS... -> target/ship/LABEL-tabs.txt; its exit status
tabs() {
  local label=$1; shift
  python3 -I scripts/parity-tabs.py "$@" --out "$OUT/tabs-$label" >"$OUT/$label-tabs.txt" 2>&1
  local st=$?
  cat "$OUT/$label-tabs.txt"
  return "$st"
}

if [ "$LIVE_ONLY" = 0 ]; then
  say "candidate: feature journeys on a box built at dev (${H_DEV:0:12})"
  BOX_DEV=$(build_box "$H_DEV" dev) || die "building the box at dev failed ($OUT/build-dev.log)"
  [ -x "$RELAY_DEV" ] || die "no relay built at agent-relay's dev tip ($RELAY_DEV): compat.sh builds it"
  parity candidate "$BOX_DEV" --relay-bin "$RELAY_DEV"; c1=$?
  say "candidate: every tab on dev's playground page"
  tabs candidate --dist "$DEV_DIR/docs" --path /playground; c2=$?
  if [ "$c1" != 0 ] || [ "$c2" != 0 ]; then
    echo "ship.sh: a feature fails on the candidate itself; fix it on dev first. Nothing promoted." >&2
    exit 1
  fi
fi

# ---- 2. promote ------------------------------------------------------------

ahead() { gh api "repos/$(slug "$1")/compare/$(default_of "$1")...$(candidate "$1")" --jq .ahead_by 2>/dev/null; }

if [ "$LIVE_ONLY" = 0 ]; then
  say "the promotion"
  TODO=""
  for r in $REPOS_ALL; do
    n=$(ahead "$r")
    [ -n "$n" ] || die "GitHub did not say how far $r's dev is ahead"
    printf '  %-18s %s is %s commits ahead of %s\n' "$r" "$(candidate "$r" | cut -c1-12)" "$n" "$(default_of "$r")"
    [ "$n" = 0 ] || TODO="$TODO $r"
  done
  if [ -z "$TODO" ]; then
    echo "  nothing to promote"
  elif [ "$DRY" = 1 ]; then
    echo "ship.sh: --dry-run; the candidate is green and nothing was promoted"
    exit 0
  else
    if [ "$YES" = 0 ]; then
      printf '\nPromote%s to their default branches and deploy the relay? Type ship: ' "$TODO"
      read -r answer
      [ "$answer" = ship ] || { echo "ship.sh: not promoted"; exit 3; }
    fi
    stamp=$(date -u +%Y-%m-%dT%H%MZ)
    for r in $TODO; do
      s=$(slug "$r"); base=$(default_of "$r"); sha=$(candidate "$r"); br=ship/$(echo "$stamp" | tr -d ':-')
      # A branch at the checked commit, so a merge into dev since is not shipped with it.
      gh api "repos/$s/git/refs" -f ref="refs/heads/$br" -f sha="$sha" >/dev/null || die "creating $br at ${sha:0:12} in $s failed"
      url=$(gh pr create --repo "$s" --base "$base" --head "$br" --title "Promote dev to $base ($stamp)" \
        --body "make ship: the candidate ${sha:0:12} from dev, parity green (helmsman target/ship/report.md)." 2>&1 | tail -1)
      case "$url" in https://*) ;; *) die "opening the $r promotion failed: $url" ;; esac
      echo "  $r: $url (${sha:0:12})"
      gh pr merge "$url" --merge >/dev/null 2>&1 || gh pr merge "$url" --merge --auto >/dev/null 2>&1 || die "merging $url failed"
      wait_merged "$url" 30 || die "$url did not merge in 30 min"
      gh api -X DELETE "repos/$s/git/refs/heads/$br" >/dev/null 2>&1 || true
      echo "$r $(gh pr view "$url" --json mergeCommit --jq .mergeCommit.oid)" >>"$OUT/promoted.txt"
    done
    case " $TODO " in
      *" agent-relay "*)
        say "deploy the relay from main"
        rel=$(src_of agent-relay); wt=$rel/.claude/worktrees/ship-main
        git -C "$rel" fetch -q origin main || die "fetching agent-relay failed"
        git -C "$rel" worktree add -q --detach "$wt" origin/main || die "a worktree of agent-relay at main failed"
        (cd "$wt" && make deploy) >"$OUT/relay-deploy.log" 2>&1; d=$?
        git -C "$rel" worktree remove --force "$wt"
        [ "$d" = 0 ] || { tail -30 "$OUT/relay-deploy.log" >&2; die "the relay deploy failed ($OUT/relay-deploy.log); helmsman is promoted, the relay is not"; }
        ;;
    esac
  fi
fi

# ---- 3. live ---------------------------------------------------------------

say "waiting for live (scripts/live-state.sh at the candidate; the try box is not part of it)"
AT=""
for r in helmsman agent-relay helmsman-devices; do
  c=$(candidate "$r"); [ -n "$c" ] && AT="$AT --at $r=$c"
done
live=0
for _ in $(seq 1 40); do
  # shellcheck disable=SC2086 # AT is a list of words
  scripts/live-state.sh $AT >"$OUT/live-state.txt" 2>&1
  if ! grep -v 'try box' "$OUT/live-state.txt" | grep -q -E '^(NOT LIVE|unknown) '; then live=1; break; fi
  sleep 30
done
cat "$OUT/live-state.txt"
[ "$live" = 1 ] || die "not live after 20 min (above); check again with scripts/ship.sh --live-only"

say "live: feature journeys on a box built at master, held to dev's features"
git -C "$MAIN" fetch -q origin master
H_LIVE=$(git -C "$MAIN" rev-parse origin/master)
BOX_LIVE=$(build_box "$H_LIVE" live) || die "building the box at master failed ($OUT/build-live.log)"
[ -n "$LIVE_URL" ] && [ -r "$LIVE_TOKEN" ] || die "live parity needs the deployed relay: COMPAT_LIVE_URL and a readable COMPAT_LIVE_TOKEN ($LIVE_TOKEN)"
parity live "$BOX_LIVE" --url "$LIVE_URL" --token "$LIVE_TOKEN"
say "live: every tab on $SITE/playground/"
tabs live --site "$SITE"

fi

# ---- 4. report -------------------------------------------------------------

# outcome FILE CHECK -> ok | FAIL | pending
outcome() {
  python3 -I - "$1" "$2" <<'PY'
import json, sys
try:
    rows = json.load(open(sys.argv[1]))
except Exception:
    print("missing"); sys.exit()
for r in rows:
    if r["check"] == sys.argv[2]:
        print("ok" if r["ok"] else "FAIL"); sys.exit()
print("pending")
PY
}
tab_outcome() { grep -q -E "^ok +tab:$2\$" "$1" 2>/dev/null && echo ok || echo FAIL; }

{
  echo "# make ship, $(date -u +%Y-%m-%dT%H:%MZ)"
  echo
  echo "candidate: helmsman dev ${H_DEV:0:12}; live: master ${H_LIVE:0:12}, $SITE"
  echo
  echo "| feature check | candidate | live |"
  echo "|---|---|---|"
} >"$OUT/report.md"
regress=""
for j in $(grep -o -E '^\| [^|]+ \| `[a-z0-9-]+`' "$FEATURES" | sed -E 's/.*`([a-z0-9-]+)`/\1/'); do
  c=$(outcome "$OUT/candidate.json" "parity/$j"); l=$(outcome "$OUT/live.json" "parity/$j")
  echo "| parity/$j | $c | $l |" >>"$OUT/report.md"
  [ "$c" = ok ] && [ "$l" != ok ] && regress="$regress parity/$j"
done
for t in $(grep -o -E '`tab:[a-z0-9-]+`' "$FEATURES" | tr -d '`' | sed 's/tab://' | sort -u); do
  c=$(tab_outcome "$OUT/candidate-tabs.txt" "$t"); l=$(tab_outcome "$OUT/live-tabs.txt" "$t")
  echo "| tab:$t | $c | $l |" >>"$OUT/report.md"
  [ "$c" = ok ] && [ "$l" != ok ] && regress="$regress tab:$t"
done
say "report ($OUT/report.md)"
cat "$OUT/report.md"

if [ -z "$regress" ]; then
  echo
  echo "ship.sh: live matches the candidate, feature by feature."
  exit 0
fi

echo
echo "ship.sh: REGRESSED on live:$regress"
[ "$REPORT_ONLY" = 1 ] && exit 1
merge=$(awk '$1 == "helmsman" { print $2 }' "$OUT/promoted.txt" 2>/dev/null | tail -1)
if [ -n "$merge" ]; then
  br=revert/ship-$(date -u +%Y%m%d%H%M)
  wt=$MAIN/.claude/worktrees/$(echo "$br" | tr '/' '-')
  git -C "$MAIN" worktree add -q -b "$br" "$wt" origin/master &&
    git -C "$wt" revert --no-edit -m 1 "$merge" >/dev/null &&
    git -C "$wt" push -q -u origin "$br" &&
    gh pr create --repo "$(slug helmsman)" --draft --base master --head "$br" \
      --title "Revert the promotion of $(git -C "$MAIN" log -1 --format=%cs "$merge"): live regressed" \
      --body "make ship found features that pass on the candidate and fail live:$regress. Report: target/ship/report.md in the checkout that ran it. Merge to take master back to before the promotion, or close and fix forward. Opened as a draft by scripts/ship.sh; nothing was reset or force-pushed." ||
    echo "ship.sh: opening the revert pull request failed; revert by hand: git revert -m 1 $merge on master" >&2
  git -C "$MAIN" worktree remove --force "$wt" 2>/dev/null
else
  echo "ship.sh: helmsman was not promoted by this run, so there is nothing of its to revert; the regression is in the relay, the device library or the site." >&2
fi
exit 1
