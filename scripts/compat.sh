#!/usr/bin/env bash
# The release train (docs/COMPAT.md): helmsman, agent-relay,
# helmsman-devices and helmsman-hardware checked against each other at
# the refs a promotion would ship, so all four can go to their default
# branches together.
#
#   scripts/compat.sh                 every contract on its three corners
#   scripts/compat.sh --candidate     only the four dev tips together
#   scripts/compat.sh --live          also the deployed relay (one short
#                                     model turn per box ref is spent)
#   scripts/compat.sh --sync          only clone (COMPAT_CLONE=1) and fetch
#                                     the sibling repositories, then stop
#   REF overrides: --helmsman R --relay R --devices R --hardware R (the
#   candidate side; default origin/dev each)
#
# Corners, per contract A<->B: A@dev + B@dev (the candidate), A@dev +
# B@live, A@live + B@dev, where "live" is the default branch (helmsman
# master, agent-relay main, helmsman-devices main, helmsman-hardware
# master). Each repository is cloned (--shared, read-only to the source)
# under target/compat/src at the exact commit, the box and the relay are
# built there, and helmsman-compat (this checkout) runs each cell. The
# report is target/compat/report.md; exit 1 when any check failed, 2 when
# a cell could not run.
#
# The sibling repositories are found beside the main checkout
# (COMPAT_REPOS overrides). Needs: cargo, git, gh (CI status; skipped
# with a note when gh is not signed in).
#
# In CI (.github/workflows/compat.yml) there are no siblings:
# COMPAT_CLONE=1 clones the missing ones from github.com into COMPAT_REPOS,
# and COMPAT_RELAY_TOKEN (a fine-grained token that reads the private
# agent-relay and helmsman-hardware) is used for those two only, as an
# http.extraheader in the environment of their git and gh commands. It is never put in a URL, a
# git config file or an argument list, and is unset before any build runs.
#
# CI still running on a candidate commit is waited for, up to COMPAT_CI_WAIT
# seconds (default 1800; 0 judges it at once, as not ready).
set -euo pipefail
cd "$(dirname "$0")/.."
ROOT=$(pwd)

RELAY_TOKEN=${COMPAT_RELAY_TOKEN:-}
unset COMPAT_RELAY_TOKEN

CANDIDATE_ONLY=0
LIVE=0
SYNC_ONLY=0
H_REF=origin/dev R_REF=origin/dev D_REF=origin/dev W_REF=origin/dev
while [ $# -gt 0 ]; do
  case "$1" in
    --candidate) CANDIDATE_ONLY=1 ;;
    --live) LIVE=1 ;;
    --sync) SYNC_ONLY=1 ;;
    --helmsman) H_REF=$2; shift ;;
    --relay) R_REF=$2; shift ;;
    --devices) D_REF=$2; shift ;;
    --hardware) W_REF=$2; shift ;;
    -h|--help) sed -n '2,34p' "$0"; exit 0 ;;
    *) echo "compat.sh: unknown argument $1 (see --help)" >&2; exit 2 ;;
  esac
  shift
done

MAIN=$(cd "$(git rev-parse --git-common-dir)/.." && pwd)
REPOS=${COMPAT_REPOS:-$(dirname "$MAIN")}
OUT=$ROOT/target/compat
mkdir -p "$OUT/src" "$OUT/build" "$OUT/cells"
rm -rf "$OUT/cells"/* "$OUT/work"
mkdir -p "$OUT/work"
LIVE_URL=${COMPAT_LIVE_URL:-}
LIVE_TOKEN=${COMPAT_LIVE_TOKEN:-$HOME/github/helmsman-try/scratch/relay-token}

src_of() { # repo -> source checkout
  case "$1" in
    helmsman) echo "$MAIN" ;;
    *) echo "$REPOS/$1" ;;
  esac
}
live_branch() {
  case "$1" in
    helmsman|helmsman-hardware) echo origin/master ;;
    *) echo origin/main ;;
  esac
}
slug() { # GitHub owner/name of a checkout
  git -C "$1" remote get-url origin | sed -E 's#(git@github.com:|https://github.com/)##; s#\.git$##'
}

# The private repositories: the token is used for these and no other.
is_private() { case "$1" in agent-relay|helmsman-hardware) return 0 ;; *) return 1 ;; esac; }

# gitr REPO ARGS...: git, with the token for a private repository as an
# extra header for this command only.
gitr() {
  local repo=$1; shift
  if is_private "$repo" && [ -n "$RELAY_TOKEN" ]; then
    local h; h=$(printf 'x-access-token:%s' "$RELAY_TOKEN" | base64 | tr -d '\n')
    GIT_TERMINAL_PROMPT=0 GIT_CONFIG_COUNT=1 GIT_CONFIG_KEY_0=http.https://github.com/.extraheader \
      GIT_CONFIG_VALUE_0="AUTHORIZATION: basic $h" git "$@"
  else
    GIT_TERMINAL_PROMPT=0 git "$@"
  fi
}

# ghr REPO ARGS...: gh, signed in with the token for a private repository.
ghr() {
  local repo=$1; shift
  if is_private "$repo" && [ -n "$RELAY_TOKEN" ]; then
    GH_TOKEN=$RELAY_TOKEN gh "$@"
  else
    gh "$@"
  fi
}

for r in helmsman agent-relay helmsman-devices helmsman-hardware; do
  if [ ! -e "$(src_of $r)/.git" ] && [ "${COMPAT_CLONE:-0}" = 1 ] && [ "$r" != helmsman ]; then
    echo "  clone $r into $REPOS"
    mkdir -p "$REPOS"
    gitr "$r" clone -q "https://github.com/JimothyJohn/$r.git" "$(src_of $r)" || { echo "compat.sh: cloning $r failed" >&2; exit 2; }
  fi
  [ -e "$(src_of $r)/.git" ] || { echo "compat.sh: no checkout of $r at $(src_of $r)" >&2; exit 2; }
  gitr "$r" -C "$(src_of $r)" fetch -q origin || { echo "compat.sh: fetching $r failed" >&2; exit 2; }
done
[ "$SYNC_ONLY" = 0 ] || exit 0

sha() { git -C "$(src_of "$1")" rev-parse --verify -q "$2^{commit}"; }

# checkout REPO SHA -> path of a clone at that commit
checkout() {
  local dir="$OUT/src/$1@${2:0:12}"
  if [ ! -d "$dir/.git" ]; then
    rm -rf "$dir"
    git clone -q --shared --no-checkout "$(src_of "$1")" "$dir"
    git -C "$dir" checkout -q --detach "$2"
  fi
  echo "$dir"
}

# build REPO SHA LABEL -> path of the program (helmsman-plc or agent-relay-local)
build() {
  local dir; dir=$(checkout "$1" "$2")
  local pkg; case "$1" in helmsman) pkg=helmsman-plc ;; agent-relay) pkg=agent-relay-local ;; esac
  local target="$OUT/build/$1-$3"
  echo "  build $pkg at $1@${2:0:12} ($3)" >&2
  (cd "$dir" && CARGO_TARGET_DIR="$target" cargo build -q --locked -p "$pkg" >"$OUT/build/$1-$3.log" 2>&1) || {
    echo "compat.sh: building $pkg at $1@${2:0:12} failed; see $OUT/build/$1-$3.log" >&2
    tail -20 "$OUT/build/$1-$3.log" >&2
    exit 2
  }
  echo "$target/debug/$pkg"
}

H_DEV=$(sha helmsman "$H_REF"); H_LIVE=$(sha helmsman "$(live_branch helmsman)")
R_DEV=$(sha agent-relay "$R_REF"); R_LIVE=$(sha agent-relay "$(live_branch agent-relay)")
D_DEV=$(sha helmsman-devices "$D_REF"); D_LIVE=$(sha helmsman-devices "$(live_branch helmsman-devices)")
W_DEV=$(sha helmsman-hardware "$W_REF"); W_LIVE=$(sha helmsman-hardware "$(live_branch helmsman-hardware)")

echo "== the train"
printf '  %-18s candidate %s (%s)  live %s\n' \
  helmsman "${H_DEV:0:12}" "$H_REF" "${H_LIVE:0:12}" \
  agent-relay "${R_DEV:0:12}" "$R_REF" "${R_LIVE:0:12}" \
  helmsman-devices "${D_DEV:0:12}" "$D_REF" "${D_LIVE:0:12}" \
  helmsman-hardware "${W_DEV:0:12}" "$W_REF" "${W_LIVE:0:12}"
# The candidate as full commits, for scripts/ship.sh: it promotes these, not
# whatever dev holds by the time it gets there.
mkdir -p "$OUT"
printf '%s %s\n' helmsman "$H_DEV" agent-relay "$R_DEV" \
  helmsman-devices "$D_DEV" helmsman-hardware "$W_DEV" >"$OUT/train.txt"

echo "== build"
(cargo build -q --locked -p helmsman-compat) || exit 2
CHECK=$ROOT/target/debug/helmsman-compat
H_DEV_DIR=$(checkout helmsman "$H_DEV"); BOX_DEV=$(build helmsman "$H_DEV" dev)
R_DEV_BIN=$(build agent-relay "$R_DEV" dev)
D_DEV_DIR=$(checkout helmsman-devices "$D_DEV")
W_DEV_DIR=$(checkout helmsman-hardware "$W_DEV")
if [ "$CANDIDATE_ONLY" = 0 ]; then
  H_LIVE_DIR=$(checkout helmsman "$H_LIVE"); BOX_LIVE=$(build helmsman "$H_LIVE" live)
  R_LIVE_BIN=$(build agent-relay "$R_LIVE" live)
  D_LIVE_DIR=$(checkout helmsman-devices "$D_LIVE")
  W_LIVE_DIR=$(checkout helmsman-hardware "$W_LIVE")
fi

STATUS=0
ROWS=""
# cell NAME ARGS... : runs one helmsman-compat cell, keeps its output in
# cells/<file>.{json,txt}, the file named from NAME with only letters,
# digits, dots, dashes and underscores ("devices: box@dev + devices@dev" is
# devices_box-dev_devices-dev): upload-artifact refuses a colon, and no
# file name here has a space.
cell() {
  local name=$1; shift
  local file; file=$(printf '%s' "$name" | tr '@' '-' | tr -cs 'A-Za-z0-9.-' '_')
  echo "== $name"
  local rc=0
  "$CHECK" "$@" --json "$OUT/cells/$file.json" >"$OUT/cells/$file.txt" 2>&1 || rc=$?
  grep -E '^(FAIL|[0-9]+ checks)' "$OUT/cells/$file.txt" || tail -5 "$OUT/cells/$file.txt"
  local verdict=pass
  case $rc in 0) ;; 1) verdict=FAIL; [ $STATUS -lt 1 ] && STATUS=1 ;; *) verdict="could not run"; STATUS=2 ;; esac
  ROWS="$ROWS| $name | $verdict | $(tail -1 "$OUT/cells/$file.txt") |
"
}

W="--work $OUT/work"
cell "devices: box@dev + devices@dev" devices --box "$BOX_DEV" --helmsman "$H_DEV_DIR" --devices "$D_DEV_DIR" $W
cell "relay: box@dev + relay@dev" relay --box "$BOX_DEV" --helmsman "$H_DEV_DIR" --relay-bin "$R_DEV_BIN" --latest $W
cell "hardware: firmware@dev + board@dev" hardware --helmsman "$H_DEV_DIR" --hardware "$W_DEV_DIR"
if [ "$CANDIDATE_ONLY" = 0 ]; then
  cell "devices: box@dev + devices@live" devices --box "$BOX_DEV" --helmsman "$H_DEV_DIR" --devices "$D_LIVE_DIR" $W
  cell "devices: box@live + devices@dev" devices --box "$BOX_LIVE" --helmsman "$H_LIVE_DIR" --devices "$D_DEV_DIR" $W
  cell "relay: box@dev + relay@live" relay --box "$BOX_DEV" --helmsman "$H_DEV_DIR" --relay-bin "$R_LIVE_BIN" $W
  cell "relay: box@live + relay@dev" relay --box "$BOX_LIVE" --helmsman "$H_LIVE_DIR" --relay-bin "$R_DEV_BIN" $W
  cell "hardware: firmware@dev + board@live" hardware --helmsman "$H_DEV_DIR" --hardware "$W_LIVE_DIR"
  cell "hardware: firmware@live + board@dev" hardware --helmsman "$H_LIVE_DIR" --hardware "$W_DEV_DIR"
fi
if [ "$LIVE" = 1 ]; then
  if [ -z "$LIVE_URL" ]; then
    # The deployed relay the try box uses, unless COMPAT_LIVE_URL says.
    LIVE_URL=$(grep -oE 'https://[a-z0-9.-]+\.on\.aws' "$HOME/github/helmsman-try/scratch/run-box.sh" 2>/dev/null | head -1 || true)
  fi
  if [ -z "$LIVE_URL" ] || [ ! -r "$LIVE_TOKEN" ]; then
    echo "compat.sh: --live needs COMPAT_LIVE_URL and a readable COMPAT_LIVE_TOKEN ($LIVE_TOKEN)" >&2
    STATUS=2
  else
    cell "live: box@dev + deployed relay" live --box "$BOX_DEV" --helmsman "$H_DEV_DIR" --url "$LIVE_URL" --token "$LIVE_TOKEN" $W
    [ "$CANDIDATE_ONLY" = 0 ] && cell "live: box@live + deployed relay" live --box "$BOX_LIVE" --helmsman "$H_LIVE_DIR" --url "$LIVE_URL" --token "$LIVE_TOKEN" $W
  fi
fi

# CI on each candidate commit: a promotion of a red commit is not ready
# whatever the contracts say.
echo "== CI on the candidate commits"
CI=""
for pair in "helmsman $H_DEV" "agent-relay $R_DEV" "helmsman-devices $D_DEV" "helmsman-hardware $W_DEV"; do
  set -- $pair
  # Each workflow's newest run on that exact commit that came to a
  # verdict: a cancelled run is none, wherever it falls in time. The ship
  # of 2026-11-22 opened its promote PR on hardware's dev tip, the PR's CI
  # run was cancelled when it merged, and as the newest run on that commit
  # it hid the green push run and refused the ship of 2026-11-24. A
  # workflow whose every run was cancelled still reads cancelled. A merge
  # that auto-merge
  # made (GITHUB_TOKEN) fires no push run on dev (automerge.yml says so),
  # so a merge commit with none is judged by its PR's head, its second
  # parent, and says so: the merged tree itself was not re-run there (the
  # contracts above were run on it here).
  runs() {
    ghr "$1" run list -R "$(slug "$(src_of "$1")")" --commit "$2" --limit 20 \
      --json workflowName,conclusion,status,createdAt \
      --jq 'group_by(.workflowName) | map((map(select(.conclusion != "cancelled")) | max_by(.createdAt)) // max_by(.createdAt)) | map(.workflowName + "=" + (if .status == "completed" then .conclusion else .status end)) | join(" ")' \
      2>/dev/null
  }
  # A run still going is waited for (COMPAT_CI_WAIT seconds, default 30 min)
  # rather than judged: while other sessions merge into dev, a ship nearly
  # always meets a tip whose CI has not finished (2026-11-21, three times).
  waited=0
  while :; do
    line=$(runs "$1" "$2") || line="unknown: gh could not list runs (signed in?)"
    case "$line" in *=in_progress*|*=queued*|*=waiting*|*=pending*|*=requested*) ;; *) break ;; esac
    [ "$waited" -lt "${COMPAT_CI_WAIT:-1800}" ] || break
    [ "$waited" = 0 ] && echo "  $1@${2:0:12}: $line; waiting for it" >&2
    sleep 30; waited=$((waited + 30))
  done
  if [ -z "$line" ]; then
    head=$(git -C "$(src_of "$1")" rev-parse -q --verify "$2^2" 2>/dev/null || true)
    if [ -n "$head" ] && hl=$(runs "$1" "$head") && [ -n "$hl" ]; then
      line="$hl (on its PR head ${head:0:12}; the merge commit itself has no run)"
    else
      line="none: no workflow ran on this commit, so it is unverified"
    fi
  fi
  echo "  $1@${2:0:12}: $line"
  CI="$CI| $1 | \`${2:0:12}\` | $line |
"
  case "$line" in
    *=success*) case "$line" in *=failure*|*=cancelled*|*=timed_out*|*=in_progress*|*=queued*) [ $STATUS -lt 1 ] && STATUS=1 ;; esac ;;
    *) [ $STATUS -lt 1 ] && STATUS=1 ;;
  esac
done

{
  echo "# Release train, $(date '+%Y-%m-%d %H:%M %Z')"
  echo
  echo "| repository | candidate | live |"
  echo "|---|---|---|"
  echo "| helmsman | \`${H_DEV:0:12}\` ($H_REF) | \`${H_LIVE:0:12}\` |"
  echo "| agent-relay | \`${R_DEV:0:12}\` ($R_REF) | \`${R_LIVE:0:12}\` |"
  echo "| helmsman-devices | \`${D_DEV:0:12}\` ($D_REF) | \`${D_LIVE:0:12}\` |"
  echo "| helmsman-hardware | \`${W_DEV:0:12}\` ($W_REF) | \`${W_LIVE:0:12}\` |"
  echo
  echo "| cell | verdict | checks |"
  echo "|---|---|---|"
  printf '%s' "$ROWS"
  echo
  echo "| CI | commit | check runs |"
  echo "|---|---|---|"
  printf '%s' "$CI"
  echo
  echo "## Failures"
  echo
  grep -h '^FAIL' "$OUT"/cells/*.txt | sort | uniq -c | sed 's/^ *\([0-9]*\) /- (x\1) /' || echo "none"
} >"$OUT/report.md"

echo "== verdict"
case $STATUS in
  0) if [ "$CANDIDATE_ONLY" = 1 ]; then
       echo "the dev tips are ready together: every contract holds on the candidate corner and CI is green; the skew corners (promotion order) were not run (report: $OUT/report.md)"
     else
       echo "the train is ready: every contract holds on every corner and CI is green (report: $OUT/report.md)"
     fi ;;
  1) echo "NOT READY: see the FAIL lines above and $OUT/report.md (each cell's full output: $OUT/cells/)" ;;
  *) echo "INCOMPLETE: a cell could not run (see $OUT/cells/)" ;;
esac
exit $STATUS
