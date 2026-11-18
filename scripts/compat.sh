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
set -euo pipefail
cd "$(dirname "$0")/.."
ROOT=$(pwd)

CANDIDATE_ONLY=0
LIVE=0
H_REF=origin/dev R_REF=origin/dev D_REF=origin/dev W_REF=origin/dev
while [ $# -gt 0 ]; do
  case "$1" in
    --candidate) CANDIDATE_ONLY=1 ;;
    --live) LIVE=1 ;;
    --helmsman) H_REF=$2; shift ;;
    --relay) R_REF=$2; shift ;;
    --devices) D_REF=$2; shift ;;
    --hardware) W_REF=$2; shift ;;
    -h|--help) sed -n '2,25p' "$0"; exit 0 ;;
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

for r in helmsman agent-relay helmsman-devices helmsman-hardware; do
  [ -d "$(src_of $r)/.git" ] || [ -f "$(src_of $r)/.git" ] || { echo "compat.sh: no checkout of $r at $(src_of $r)" >&2; exit 2; }
  git -C "$(src_of $r)" fetch -q origin
done

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
# cell NAME ARGS... : runs one helmsman-compat cell, keeps its output
cell() {
  local name=$1; shift
  echo "== $name"
  local rc=0
  "$CHECK" "$@" --json "$OUT/cells/$name.json" >"$OUT/cells/$name.txt" 2>&1 || rc=$?
  grep -E '^(FAIL|[0-9]+ checks)' "$OUT/cells/$name.txt" || tail -5 "$OUT/cells/$name.txt"
  local verdict=pass
  case $rc in 0) ;; 1) verdict=FAIL; [ $STATUS -lt 1 ] && STATUS=1 ;; *) verdict="could not run"; STATUS=2 ;; esac
  ROWS="$ROWS| $name | $verdict | $(tail -1 "$OUT/cells/$name.txt") |
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
  # Each workflow's newest run on that exact commit (an older run a
  # re-run or a reopen cancelled does not count). A merge that auto-merge
  # made (GITHUB_TOKEN) fires no push run on dev (automerge.yml says so),
  # so a merge commit with none is judged by its PR's head, its second
  # parent, and says so: the merged tree itself was not re-run there (the
  # contracts above were run on it here).
  runs() {
    gh run list -R "$(slug "$(src_of "$1")")" --commit "$2" --limit 20 \
      --json workflowName,conclusion,status,createdAt \
      --jq 'group_by(.workflowName) | map(max_by(.createdAt)) | map(.workflowName + "=" + (if .status == "completed" then .conclusion else .status end)) | join(" ")' \
      2>/dev/null
  }
  line=$(runs "$1" "$2") || line="unknown: gh could not list runs (signed in?)"
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
