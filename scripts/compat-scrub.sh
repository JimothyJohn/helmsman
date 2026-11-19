#!/usr/bin/env bash
# Fails when the relay token (COMPAT_RELAY_TOKEN, or the Basic form
# scripts/compat.sh derives from it) appears in anything compat leaves
# behind: the report, the cells, the run log, the build logs, the clones'
# git config. Run by .github/workflows/compat.yml before anything is
# uploaded; the token is read from the environment and never printed.
#
#   COMPAT_RELAY_TOKEN=... scripts/compat-scrub.sh [OUT [REPOS]]
#
# OUT defaults to target/compat, REPOS to $COMPAT_REPOS. Without a token
# there is nothing to look for and it exits 0.
set -euo pipefail
cd "$(dirname "$0")/.."

TOKEN=${COMPAT_RELAY_TOKEN:-}
if [ -z "$TOKEN" ]; then
  echo "compat-scrub: no token set, nothing to look for"
  exit 0
fi
OUT=${1:-target/compat}
REPOS=${2:-${COMPAT_REPOS:-}}
B64=$(printf 'x-access-token:%s' "$TOKEN" | base64 | tr -d '\n')

targets=()
[ -d "$OUT" ] && targets+=("$OUT")
for f in "$OUT"/build/*.log; do [ -e "$f" ] && targets+=("$f"); done
for f in .git/config "$REPOS"/*/.git/config; do [ -e "$f" ] && targets+=("$f"); done
[ "${#targets[@]}" -gt 0 ] || { echo "compat-scrub: nothing to scan"; exit 0; }

# -f reads the patterns from a descriptor, so neither value is in an
# argument list; -l names the files and never prints a match.
hits=$(grep -rlF --exclude-dir=build --exclude-dir=objects \
  -f <(printf '%s\n%s\n' "$TOKEN" "$B64") -- "${targets[@]}" 2>/dev/null || true)
if [ -n "$hits" ]; then
  echo "compat-scrub: the relay token (or its header form) is in these files:" >&2
  echo "$hits" >&2
  exit 1
fi
echo "compat-scrub: the token is in none of ${#targets[@]} locations scanned"
