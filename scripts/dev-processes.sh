#!/usr/bin/env bash
# Every Helmsman dev process on this machine: boxes (helmsman-plc), the
# canvas dev servers (trunk, python http.server on a canvas dir), the TODO
# board, the conformance harness and the simulators. Lists each with its
# pid, listening ports, checkout and start time.
#
#   scripts/dev-processes.sh          list
#   scripts/dev-processes.sh --stop   stop every one, except a process
#                                     running from a locked worktree (a
#                                     service checkout such as the try box,
#                                     `git worktree lock`); --all stops
#                                     those too
#
# Why: a box started with nohup outlives the session that started it, and
# one ran for five days on :8080 with nobody using it (2026-11-07). Run
# this at the end of a session, or whenever a port is busy.
set -euo pipefail

mode="${1:-list}"
case "$mode" in list | --stop | --all) ;; *)
  echo "usage: $0 [--stop | --all]" >&2
  exit 2
  ;;
esac

repo="$(git -C "$(dirname "$0")/.." rev-parse --show-toplevel)"
locked="$(git -C "$repo" worktree list --porcelain | awk '
  /^worktree /{ p = substr($0, 10) } /^locked/{ print p }')"

# pid and command of each candidate (one per line: "PID COMMAND...").
candidates() {
  ps -axo pid=,command= | awk '
    /awk|dev-processes\.sh/ { next }
    /helmsman-plc|helmsman-node|\/trunk (serve|watch)|todo_board\.py|examples\/conformance|examples\/(sim|modbus_sim|enip_sim)/ { print; next }
    /http\.server/ && /canvas-dist|docs\/playground|helmsman/ { print }'
}

n=0
stopped=0
kept=0
while read -r pid cmd; do
  [ -n "$pid" ] || continue
  n=$((n + 1))
  cwd="$(lsof -a -p "$pid" -d cwd -Fn 2>/dev/null | sed -n 's/^n//p' | head -1)"
  ports="$(lsof -a -p "$pid" -nP -iTCP -sTCP:LISTEN -Fn 2>/dev/null | sed -n 's/^n.*://p' | sort -un | tr '\n' ' ')"
  since="$(ps -o lstart= -p "$pid" | xargs)"
  service=""
  while read -r w; do
    [ -n "$w" ] || continue
    case "$cwd/" in "$w"/*) service="$w" ;; esac
  done <<<"$locked"
  printf '%-7s ports %-12s since %s\n        %s\n        in %s%s\n' \
    "$pid" "${ports:-none}" "$since" "${cmd:0:160}" "${cwd:-?}" "${service:+ (locked service checkout)}"
  if [ "$mode" = list ]; then
    continue
  fi
  if [ -n "$service" ] && [ "$mode" != --all ]; then
    kept=$((kept + 1))
    echo "        kept: a service checkout (--all stops it)"
    continue
  fi
  kill "$pid" 2>/dev/null || true
  for _ in 1 2 3 4 5; do kill -0 "$pid" 2>/dev/null || break; sleep 1; done
  if kill -0 "$pid" 2>/dev/null; then
    kill -9 "$pid" 2>/dev/null || true
  fi
  if kill -0 "$pid" 2>/dev/null; then
    echo "        STILL RUNNING" >&2
  else
    stopped=$((stopped + 1))
    echo "        stopped"
  fi
done < <(candidates)

if [ "$n" = 0 ]; then
  echo "no Helmsman dev processes running"
elif [ "$mode" != list ]; then
  echo "stopped $stopped, kept $kept, of $n"
fi
