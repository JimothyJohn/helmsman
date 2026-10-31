#!/usr/bin/env bash
# Make (or re-password) a login on a Pi that scripts/pi-deploy-plc.sh set
# up: `helmsman-plc account add USER ROLE` (or `passwd USER` when the user
# exists) in the service's state directory, as root, then the accounts file
# handed to the service's own (DynamicUser) owner, as the box wants it
# (mode 0600, refused if others can read it). Once any account exists the
# box refuses role PINs: approvals, edits, deploys and forces come from a
# login. Safe to re-run: an existing user gets a new password.
#
# Needs: key SSH and passwordless sudo on the Pi (scripts/pi-prep.sh), the
# service installed (scripts/pi-deploy-plc.sh). The password is typed on the
# Pi's terminal (ssh -t, not echoed) and piped to the binary's stdin; it is
# never an argument or an environment variable, here or there.
# Runs here or in the native terminal: `! scripts/pi-account.sh nick@<pi-ip> nick engineer`
# (the `!` prompt has a terminal for the password).
#
# Usage: scripts/pi-account.sh user@host USER [ROLE]
#        ROLE: operator | engineer (default) | integrator
set -euo pipefail

target="${1:?usage: $0 user@host USER [operator|engineer|integrator]}"
user="${2:?usage: $0 user@host USER [operator|engineer|integrator]}"
role="${3:-engineer}"
case "$role" in
    operator | engineer | integrator) ;;
    *) echo "ROLE must be operator, engineer or integrator: $role" >&2; exit 2 ;;
esac
case "$user" in
    *[!A-Za-z0-9_.-]* | "") echo "USER is letters, digits, _ . -: $user" >&2; exit 2 ;;
esac
state=/var/lib/helmsman-plc

ssh -o BatchMode=yes "$target" true || { echo "cannot ssh to $target with a key; run scripts/pi-prep.sh first" >&2; exit 1; }
ssh "$target" 'sudo -n true' || { echo "sudo on $target needs a password; see scripts/pi-prep.sh" >&2; exit 1; }
# shellcheck disable=SC2029 # $state is meant to expand there.
ssh "$target" "test -x /usr/local/bin/helmsman-plc && sudo test -d $state" \
    || { echo "no helmsman-plc service on $target; run scripts/pi-deploy-plc.sh first" >&2; exit 1; }

# shellcheck disable=SC2029 # $state, $user and $role are meant to expand there.
ssh -t "$target" "set -e
plc() { sudo env STATE_DIRECTORY=$state /usr/local/bin/helmsman-plc account \"\$@\"; }
printf 'password for $user ($role): '
IFS= read -rs pw
echo
[ -n \"\$pw\" ] || { echo 'empty password: nothing changed' >&2; exit 1; }
if plc list | awk '{print \$1}' | grep -qx '$user'; then
    printf '%s\\n' \"\$pw\" | plc passwd '$user'
else
    printf '%s\\n' \"\$pw\" | plc add '$user' '$role'
fi
unset pw
sudo chown --reference=$state $state/accounts
sudo chmod 600 $state/accounts"

echo "==> verify"
# shellcheck disable=SC2029 # $state and $user are meant to expand there.
ssh "$target" "sudo env STATE_DIRECTORY=$state /usr/local/bin/helmsman-plc account list | grep -w '$user'" \
    || { echo "FAIL: $user is not in the accounts on $target" >&2; exit 1; }
echo "$user can sign in from the editor's header on $target (the file is read on every login: no restart)."
