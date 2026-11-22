#!/bin/bash
# helmsman-firstboot: read the pairing files under helmsman/ on the boot
# partition, hand them to the box, and remove the secret from the
# partition (docs/BOX.md). Idempotent: nothing to read, nothing done.
#
#   relay-url    -> --relay URL in PLC_RELAY_ARGS (/etc/helmsman-plc/env)
#   relay-token  -> /etc/helmsman-plc/relay-token (root, 0600), which the
#                   unit loads as a systemd credential; then deleted here
#   pairing-code -> traded with the relay for a token (`helmsman-plc pair`),
#                   written where relay-token goes, then deleted here; needs
#                   relay-url and relay-box-id beside it, and is not used when
#                   a relay-token is there too (the file wins)
#   relay-box-id -> the id the operator made the code for: 1 to 64 of
#                   a-z 0-9 -, not starting with -
#   site         -> --site NAME in PLC_RELAY_ARGS (the site this box joins)
#   box-name     -> --box-name NAME in PLC_RELAY_ARGS, only with a site: the
#                   box refuses --box-name without --site and would not start
#
# Every value is held to what the box itself accepts (main.rs), because a
# refused argument stops the PLC service: a URL with no space or quote (the
# unit splits $PLC_RELAY_ARGS on whitespace), a site of 1 to 32 of
# A-Z a-z 0-9 . _ -, a box name of at most 48 of A-Z a-z 0-9 _ -.
# Each boot rebuilds the line from what is there: once the token has left
# the boot partition, the installed token file keeps --relay-token on it.
#
# HELMSMAN_FIRSTBOOT_ROOT prefixes every path (tests run it in a temp dir).
#
# A token reaches the box two ways: the operator copies one onto the card
# (agent-relay scripts/issue-box-token.sh, `relay-token`), or makes a
# one-time code (agent-relay scripts/pair-code.sh) the box trades for its
# own (`pairing-code`). Neither the code nor the token is ever printed.
#
# HELMSMAN_PLC_BIN names the box binary (tests point it at the one they built).
set -euo pipefail
# Every pattern below is over bytes. Under a UTF-8 locale a bash without
# `globasciiranges` (3.2) reads `a-f` by collation, A to E included, and
# an uppercase token passed the check.
export LC_ALL=C
root="${HELMSMAN_FIRSTBOOT_ROOT:-}"
plc="${HELMSMAN_PLC_BIN:-/usr/local/bin/helmsman-plc}"
boot="$root/boot/firmware/helmsman"
env="$root/etc/helmsman-plc/env"
token_file="$root/etc/helmsman-plc/relay-token"
# Where the unit's LoadCredential= puts it for the service.
cred=/run/credentials/helmsman-plc.service/relay-token
[ -d "$boot" ] || exit 0

# A file's first line, carriage returns and the blanks at either end
# removed. No pipe: `tr | head -1` under pipefail let `tr` die of SIGPIPE
# on a file with more after that line, and the script with it.
trim() {
    local line=""
    IFS= read -r line < "$1" || true
    line="${line//$'\r'/}"
    line="${line#"${line%%[![:space:]]*}"}"
    line="${line%"${line##*[![:space:]]}"}"
    printf '%s\n' "$line"
}

# https://HOST[:PORT][/PATH]: a shape the box's URL parser always takes.
# http:// only with a relay-allow-http file beside it: a relay on the plant's
# own network (agent-relay-local) has no certificate, and the file says the
# person who flashed the card knows the pairing code and the token then
# cross that network in the clear. The image's CI boot uses it too.
allow_http=0
[ -e "$boot/relay-allow-http" ] && allow_http=1
url_ok() {
    local rest authority host port path
    case "$1" in
        https://*) rest="${1#https://}" ;;
        http://*) [ "$allow_http" = 1 ] || return 1; rest="${1#http://}" ;;
        *) return 1 ;;
    esac
    authority="${rest%%/*}"
    path="${rest#"$authority"}"
    host="${authority%%:*}"
    case "$host" in "" | *[!A-Za-z0-9.-]*) return 1 ;; esac
    if [ "$host" != "$authority" ]; then
        port="${authority#*:}"
        case "$port" in "" | *[!0-9]*) return 1 ;; esac
        [ "${#port}" -le 5 ] && [ "$((10#$port))" -ge 1 ] && [ "$((10#$port))" -le 65535 ] || return 1
    fi
    case "$path" in *[!A-Za-z0-9._~/%-]*) return 1 ;; esac
    return 0
}

args=""
relay=""
if [ -f "$boot/relay-url" ]; then
    url="$(trim "$boot/relay-url")"
    if [ -n "$url" ]; then
        if url_ok "$url"; then
            relay="$url"
        else
            echo "helmsman-firstboot: relay-url is not https://HOST[:PORT][/PATH] (letters, digits, . - in the host; http:// needs a relay-allow-http file), ignored" >&2
        fi
    fi
fi
if [ -f "$boot/relay-token" ]; then
    token="$(trim "$boot/relay-token")"
    case "$token" in
        hmr1_*[!0-9a-f]* | hmr1_)
            echo "helmsman-firstboot: relay-token is not an hmr1_ token, ignored" >&2
            ;;
        hmr1_*)
            umask 077
            printf '%s\n' "$token" > "$token_file"
            chmod 600 "$token_file"
            # The secret leaves the boot partition (FAT, world-readable).
            shred -u "$boot/relay-token" 2>/dev/null || rm -f "$boot/relay-token"
            echo "helmsman-firstboot: relay token installed and removed from the boot partition"
            ;;
        "") ;;
        *) echo "helmsman-firstboot: relay-token is not an hmr1_ token, ignored" >&2 ;;
    esac
fi
# A pairing code, when no token is installed yet: the box trades it with
# the relay for its own. The relay counts wrong tries against the code, so
# a code the relay refused is dropped here, not retried at every boot; one
# that could not be tried (no network yet, a relay that failed) stays for
# the next start (`systemctl start helmsman-firstboot`).
if [ -f "$boot/pairing-code" ]; then
    drop_code() { shred -u "$boot/pairing-code" 2>/dev/null || rm -f "$boot/pairing-code"; }
    if [ -s "$token_file" ]; then
        echo "helmsman-firstboot: a relay token is already installed; the pairing code is not used and was removed" >&2
        drop_code
    elif [ -z "$relay" ]; then
        echo "helmsman-firstboot: pairing-code needs a valid relay-url beside it, not used yet" >&2
    else
        box_id=""
        [ -f "$boot/relay-box-id" ] && box_id="$(trim "$boot/relay-box-id")"
        case "$box_id" in
            "" | -* | *[!a-z0-9-]*)
                echo "helmsman-firstboot: pairing-code needs relay-box-id beside it (1 to 64 of a-z 0-9 -), not used yet" >&2
                ;;
            *)
                if [ "${#box_id}" -gt 64 ]; then
                    echo "helmsman-firstboot: relay-box-id is longer than 64, not used yet" >&2
                else
                    umask 077
                    rc=0
                    "$plc" pair --relay "$relay" --box-id "$box_id" \
                        --code-file "$boot/pairing-code" --out "$token_file" --wait 30 || rc=$?
                    if [ "$rc" -eq 0 ]; then
                        drop_code
                        echo "helmsman-firstboot: paired by code; the code is removed from the boot partition"
                    elif [ "$rc" -eq 3 ]; then
                        drop_code
                        echo "helmsman-firstboot: the relay refused the pairing code; it is removed. Put a new pairing-code and run: sudo systemctl start helmsman-firstboot" >&2
                    else
                        echo "helmsman-firstboot: pairing did not finish (exit $rc); the code stays for the next start" >&2
                    fi
                fi
                ;;
        esac
    fi
fi
# --relay and --relay-token go together or the box will not start. The
# token file outlives the boot partition's copy, so a later boot (the token
# already shredded there) still pairs.
if [ -n "$relay" ] && [ -s "$token_file" ]; then
    args="$args --relay $relay --relay-token $cred"
elif [ -n "$relay" ]; then
    echo "helmsman-firstboot: relay-url without a token (put relay-token beside it), the relay is not set" >&2
elif [ -f "$boot/relay-token" ] || [ -s "$token_file" ]; then
    echo "helmsman-firstboot: a relay token without relay-url, the relay is not set" >&2
fi
site=""
if [ -f "$boot/site" ]; then
    site="$(trim "$boot/site" | LC_ALL=C tr -c 'A-Za-z0-9._\n-' '-' | cut -c1-32)"
    [ -n "$site" ] && args="$args --site $site"
fi
if [ -f "$boot/box-name" ]; then
    name="$(trim "$boot/box-name" | LC_ALL=C tr -c 'A-Za-z0-9_\n-' '-' | cut -c1-48)"
    if [ -n "$name" ] && [ -n "$site" ]; then
        args="$args --box-name $name"
    elif [ -n "$name" ]; then
        echo "helmsman-firstboot: box-name needs a site file beside it (the box refuses a name without a site), ignored" >&2
    fi
fi
[ -n "$args" ] || exit 0

# Replace the PLC_RELAY_ARGS line, keeping whatever else env holds.
tmp="$(mktemp)"
grep -v '^PLC_RELAY_ARGS=' "$env" > "$tmp" 2>/dev/null || true
printf 'PLC_RELAY_ARGS=%s\n' "${args# }" >> "$tmp"
install -m 644 "$tmp" "$env"
rm -f "$tmp"
echo "helmsman-firstboot: PLC_RELAY_ARGS=${args# }"
