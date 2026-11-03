#!/bin/bash
# helmsman-firstboot: read the pairing files under helmsman/ on the boot
# partition, hand them to the box, and remove the secret from the
# partition (docs/BOX.md). Idempotent: nothing to read, nothing done.
#
#   relay-url    -> --relay URL in PLC_RELAY_ARGS (/etc/helmsman-plc/env)
#   relay-token  -> /etc/helmsman-plc/relay-token (root, 0600), which the
#                   unit loads as a systemd credential; then deleted here
#   box-name     -> --box-name NAME in PLC_RELAY_ARGS
#
# The relay issues a token out of band today (agent-relay
# scripts/issue-box-token.sh); a pairing code the box exchanges for a token
# is relay work, noted on the box-sku card.
set -euo pipefail
boot=/boot/firmware/helmsman
env=/etc/helmsman-plc/env
token_file=/etc/helmsman-plc/relay-token
# Where the unit's LoadCredential= puts it for the service.
cred=/run/credentials/helmsman-plc.service/relay-token
[ -d "$boot" ] || exit 0

trim() { tr -d '\r' < "$1" | head -1 | sed 's/^[[:space:]]*//; s/[[:space:]]*$//'; }

args=""
if [ -f "$boot/relay-url" ]; then
    url="$(trim "$boot/relay-url")"
    case "$url" in
        https://*) args="$args --relay $url" ;;
        "") ;;
        *) echo "helmsman-firstboot: relay-url must start with https://, ignored: $url" >&2 ;;
    esac
fi
if [ -f "$boot/relay-token" ]; then
    token="$(trim "$boot/relay-token")"
    case "$token" in
        hmr1_[0-9a-f]*)
            umask 077
            printf '%s\n' "$token" > "$token_file"
            chmod 600 "$token_file"
            args="$args --relay-token $cred"
            # The secret leaves the boot partition (FAT, world-readable).
            shred -u "$boot/relay-token" 2>/dev/null || rm -f "$boot/relay-token"
            echo "helmsman-firstboot: relay token installed and removed from the boot partition"
            ;;
        "") ;;
        *) echo "helmsman-firstboot: relay-token is not an hmr1_ token, ignored" >&2 ;;
    esac
elif [ -s "$token_file" ] && ! grep -q -- '--relay-token' "$env" 2>/dev/null; then
    args="$args --relay-token $cred"
fi
if [ -f "$boot/box-name" ]; then
    name="$(trim "$boot/box-name" | tr -c 'A-Za-z0-9_\n-' '-')"
    [ -n "$name" ] && args="$args --box-name $name"
fi
[ -n "$args" ] || exit 0

# Replace the PLC_RELAY_ARGS line, keeping whatever else env holds.
tmp="$(mktemp)"
grep -v '^PLC_RELAY_ARGS=' "$env" > "$tmp" 2>/dev/null || true
printf 'PLC_RELAY_ARGS=%s\n' "${args# }" >> "$tmp"
install -m 644 "$tmp" "$env"
rm -f "$tmp"
echo "helmsman-firstboot: PLC_RELAY_ARGS=${args# }"
