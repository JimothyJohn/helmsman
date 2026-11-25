#!/usr/bin/env bash
# Give back the disk idle worktrees hold in build output (Nick, 2026-11-25:
# "I wanted to limit the dozens of GB of worktree build space"). Each
# worktree under .claude/worktrees/ builds into its own target/ (about
# 10 GB); a shared build folder was tried and broke the tests that start a
# binary (cargo hands one worktree's compiled test, with another's paths in
# it, to the next). So instead this deletes target/ where nobody is using it:
#
#   - the worktree's HEAD is in origin/dev or origin/master (its work has
#     landed), or nothing under its target/ changed for HOURS hours (12);
#   - and no process runs from it or has it as its directory (a build, a
#     test, the bench box serving its binary);
#   - and the worktree is not locked (`git worktree lock`).
#
# Only target/ goes: the code, the branch and the worktree stay, and the
# next build makes target/ again. The main checkout is never touched.
#
# Usage: scripts/reclaim-targets.sh            (or: make reclaim)
#        DRY_RUN=1 scripts/reclaim-targets.sh  lists, deletes nothing
#        HOURS=4 scripts/reclaim-targets.sh    a shorter idle time
#        scripts/reclaim-targets.sh --install  runs it daily at 04:30
#                                              (LaunchAgent, see below)
#        scripts/reclaim-targets.sh --uninstall
#
# The daily run is the LaunchAgent io.advin.helmsman-reclaim, writing to
# ~/Library/Logs/helmsman-reclaim.log. Stop it for good with --uninstall.
set -euo pipefail

hours="${HOURS:-12}"
dry="${DRY_RUN:-0}"
label="io.advin.helmsman-reclaim"
plist="$HOME/Library/LaunchAgents/$label.plist"

cd "$(git rev-parse --show-toplevel)"
main="$(git worktree list --porcelain | sed -n '1s/^worktree //p')"
root="$main/.claude/worktrees"

case "${1:-}" in
--install)
    mkdir -p "$HOME/Library/LaunchAgents" "$HOME/Library/Logs"
    cat >"$plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>$label</string>
  <key>ProgramArguments</key>
  <array><string>/bin/bash</string><string>$main/scripts/reclaim-targets.sh</string></array>
  <key>WorkingDirectory</key><string>$main</string>
  <key>StartCalendarInterval</key><dict><key>Hour</key><integer>4</integer><key>Minute</key><integer>30</integer></dict>
  <key>StandardOutPath</key><string>$HOME/Library/Logs/helmsman-reclaim.log</string>
  <key>StandardErrorPath</key><string>$HOME/Library/Logs/helmsman-reclaim.log</string>
</dict>
</plist>
EOF
    launchctl bootout "gui/$(id -u)/$label" 2>/dev/null || true
    launchctl bootstrap "gui/$(id -u)" "$plist"
    echo "installed $label: daily at 04:30, log ~/Library/Logs/helmsman-reclaim.log"
    exit 0
    ;;
--uninstall)
    launchctl bootout "gui/$(id -u)/$label" 2>/dev/null || true
    rm -f "$plist"
    echo "removed $label"
    exit 0
    ;;
"") ;;
*)
    echo "usage: $0 [--install|--uninstall]" >&2
    exit 2
    ;;
esac

echo "$(date '+%Y-%m-%d %H:%M') reclaim: idle after ${hours} h$([ "$dry" = 1 ] && echo ', dry run')"
git -C "$main" fetch -q origin 2>/dev/null || echo "  (fetch failed: merged-ness as of the last fetch)"

# What is in use: every process's directory and executable under the
# worktrees (a cargo build or test, the bench box running a worktree's
# binary, a shell sitting in one).
busy="$(lsof -nP -d cwd,txt -Fn 2>/dev/null | sed -n 's/^n//p' | grep -F "$root/" || true)"

freed=0
while IFS=$'\t' read -r path locked; do
    case "$path" in "$root"/*) ;; *) continue ;; esac
    t="$path/target"
    [ -d "$t" ] || continue
    name="${path#"$root"/}"
    if [ -n "$locked" ]; then
        echo "  keep $name: locked"
        continue
    fi
    if grep -qF "$path/" <<<"$busy"$'\n' || grep -qxF "$path" <<<"$busy"; then
        echo "  keep $name: in use"
        continue
    fi
    head="$(git -C "$path" rev-parse -q --verify HEAD 2>/dev/null || true)"
    why=""
    if [ -n "$head" ] && { git -C "$main" merge-base --is-ancestor "$head" origin/dev 2>/dev/null \
        || git -C "$main" merge-base --is-ancestor "$head" origin/master 2>/dev/null; }; then
        why="merged"
    elif [ -z "$(find "$t" -maxdepth 3 -mmin "-$((hours * 60))" -print -quit 2>/dev/null)" ]; then
        why="idle ${hours} h"
    fi
    if [ -z "$why" ]; then
        echo "  keep $name: built in the last ${hours} h"
        continue
    fi
    kb="$(du -sk "$t" 2>/dev/null | cut -f1)"
    freed=$((freed + ${kb:-0}))
    if [ "$dry" = 1 ]; then
        echo "  would clear $name ($why, $((${kb:-0} / 1024 / 1024)) GB)"
    else
        rm -rf "$t"
        echo "  cleared $name ($why, $((${kb:-0} / 1024 / 1024)) GB)"
    fi
done < <(git worktree list --porcelain | awk '
    /^worktree /{ if (p != "") print p "\t" l; p = substr($0, 10); l = "" }
    /^locked/{ l = "locked" }
    END { if (p != "") print p "\t" l }')

echo "  $([ "$dry" = 1 ] && echo 'would free' || echo freed) about $((freed / 1024 / 1024)) GB"
