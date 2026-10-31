#!/usr/bin/env bash
# Record a Pi's SSH host key in ~/.ssh/known_hosts, but only if it matches
# the fingerprint read on the Pi itself, so a first connection over the
# network is never trusted blind.
#
# Get the fingerprint on the Pi (console, or a session you already trust):
#   ssh-keygen -lf /etc/ssh/ssh_host_ed25519_key.pub
# Then here:  scripts/pi-trust-host.sh HOST SHA256:...
# Safe to re-run: does nothing if HOST's key is already known and matches.
# Needs: ssh-keyscan and ssh-keygen (OpenSSH).
set -euo pipefail

host="${1:?usage: $0 HOST SHA256:fingerprint}"
want="${2:?usage: $0 HOST SHA256:fingerprint (from ssh-keygen -lf on the Pi)}"
case "$want" in
    SHA256:*) ;;
    *) echo "fingerprint must look like SHA256:...: $want" >&2; exit 2 ;;
esac

scanned="$(ssh-keyscan -T 5 -t ed25519 "$host" 2>/dev/null)"
[ -n "$scanned" ] || { echo "no ed25519 host key from $host (is sshd up?)" >&2; exit 1; }
got="$(printf '%s\n' "$scanned" | ssh-keygen -lf - | awk '{print $2}')"
if [ "$got" != "$want" ]; then
    echo "MISMATCH: $host offers $got, the Pi says $want. Not trusting it." >&2
    exit 1
fi

if ssh-keygen -F "$host" >/dev/null 2>&1; then
    known="$(ssh-keygen -F "$host" | grep -v '^#' | ssh-keygen -lf - | awk '{print $2}' | sort -u)"
    if [ "$known" = "$want" ]; then
        echo "$host is already trusted ($want)"
        exit 0
    fi
    echo "known_hosts holds a different key for $host ($known). Remove it first:" >&2
    echo "  ssh-keygen -R $host" >&2
    exit 1
fi

printf '%s\n' "$scanned" >>"$HOME/.ssh/known_hosts"
ssh-keygen -F "$host" >/dev/null || { echo "failed to record $host" >&2; exit 1; }
echo "trusted $host ($want)"
