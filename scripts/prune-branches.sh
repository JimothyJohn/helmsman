#!/usr/bin/env bash
# Remove the worktrees and delete the branches whose work has landed.
# Worktrees first: every worktree but the main checkout and this one that
# is clean (no changes, no untracked files) and whose HEAD is in origin/dev
# or origin/master, unless it is locked (`git worktree lock --reason ...`
# keeps a service's checkout, such as the try box's ../helmsman-try).
# Branches: on origin, every branch except
# dev, master and the head of an open PR, when its PR is merged or closed.
# Locally: every branch merged into origin/dev or origin/master, or whose
# upstream is gone, except dev, master and anything a worktree has checked
# out. Then fast-forwards local dev and master, prunes stale worktree
# entries and prints what is left. Safe to re-run.
#
# Needs: gh logged in as the repo owner. Run it yourself, as
# `! ./scripts/prune-branches.sh` (deleting refs is refused to an agent).
# DRY_RUN=1 lists the deletions without making them.
set -euo pipefail

cd "$(git rev-parse --show-toplevel)"
git fetch -q --prune origin

main_tree="$(git worktree list --porcelain | sed -n '1s/^worktree //p')"
here="$(pwd -P)"
trees=()
while IFS=$'\t' read -r path locked; do
    case "$path" in "$main_tree"|"$here") continue ;; esac
    if [ -n "$locked" ]; then echo "keep worktree $path: locked"; continue; fi
    if [ ! -d "$path" ]; then continue; fi   # `git worktree prune` drops it
    if [ -n "$(git -C "$path" status --porcelain 2>/dev/null)" ]; then
        echo "keep worktree $path: uncommitted or untracked files"; continue
    fi
    head="$(git -C "$path" rev-parse HEAD)"
    if git merge-base --is-ancestor "$head" origin/dev \
        || git merge-base --is-ancestor "$head" origin/master; then
        trees+=("$path")
    else
        echo "keep worktree $path: HEAD not in origin/dev or origin/master"
    fi
done < <(git worktree list --porcelain | awk '
    /^worktree /{ if (p != "") print p "\t" l; p = substr($0, 10); l = "" }
    /^locked/{ l = "locked" }
    END { if (p != "") print p "\t" l }')

echo "worktrees: ${#trees[@]} to remove"
if [ "${DRY_RUN:-0}" = 1 ]; then
    [ ${#trees[@]} -eq 0 ] || printf '  %s\n' "${trees[@]}"
else
    for t in ${trees[@]+"${trees[@]}"}; do git worktree remove "$t"; done
fi

open_heads="$(gh pr list --state open --json headRefName --jq '.[].headRefName')"
checked_out="$(git worktree list --porcelain | sed -n 's#^branch refs/heads/##p')"

remote=()
while read -r ref; do
    [[ "$ref" == origin/* ]] || continue   # the origin/HEAD pointer prints as "origin"
    b="${ref#origin/}"
    case "$b" in HEAD|dev|master) continue ;; esac
    if grep -qxF "$b" <<<"$open_heads"; then continue; fi
    state="$(gh pr list --state all --head "$b" --json state --jq '.[0].state // empty')"
    case "$state" in
        MERGED|CLOSED) remote+=("$b") ;;
        *) echo "keep origin/$b: no merged or closed PR" ;;
    esac
done < <(git branch -r --format='%(refname:short)')

local=()
while read -r b track; do
    case "$b" in dev|master) continue ;; esac
    if grep -qxF "$b" <<<"$checked_out"; then continue; fi
    if [ "$track" = "[gone]" ] \
        || printf '%s\n' ${remote[@]+"${remote[@]}"} | grep -qxF "$b" \
        || git merge-base --is-ancestor "$b" origin/dev \
        || git merge-base --is-ancestor "$b" origin/master; then
        local+=("$b")
    else
        echo "keep $b: unmerged, upstream ${track:-none}"
    fi
done < <(git branch --format='%(refname:short) %(upstream:track)')

echo "origin: ${#remote[@]} to delete; local: ${#local[@]} to delete"
if [ "${DRY_RUN:-0}" = 1 ]; then
    # An empty array is "unbound" to bash 3.2's `set -u` (the Mac's
    # /bin/bash), and printf with no arguments still prints its format
    # once: list only what there is.
    [ ${#remote[@]} -eq 0 ] || printf '  origin/%s\n' "${remote[@]}"
    [ ${#local[@]} -eq 0 ] || printf '  %s\n' "${local[@]}"
    exit 0
fi

[ ${#remote[@]} -eq 0 ] || git push -q origin --delete "${remote[@]}"
[ ${#local[@]} -eq 0 ] || git branch -q -D "${local[@]}"

for b in dev master; do
    if grep -qxF "$b" <<<"$checked_out"; then continue; fi
    git fetch -q origin "$b:$b" || echo "local $b is not behind origin/$b; left alone"
done
git worktree prune
git fetch -q --prune origin

echo "left on origin: $(git branch -r --format='%(refname:short)' | grep '^origin/' | sed 's#^origin/##' | tr '\n' ' ')"
echo "left locally:   $(git branch --format='%(refname:short)' | tr '\n' ' ')"
