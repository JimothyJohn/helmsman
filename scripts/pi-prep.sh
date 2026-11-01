#!/usr/bin/env bash
# Prepare a fresh Pi for the box: key-only SSH, hostname, packages current,
# and a check that the EtherNet/IP ports are free, so scripts/pi-deploy-plc.sh
# can take it from there. Safe to re-run. Runs here as `! scripts/pi-prep.sh`
# once the Pi answers; the first run asks for the Pi user's password.
#
# Usage: scripts/pi-prep.sh user@host [hostname]      # default hostname: helmsman
set -euo pipefail

target="${1:?usage: $0 user@host [hostname]}"
hostname="${2:-helmsman}"
key="${HOME}/.ssh/id_ed25519.pub"
[ -f "$key" ] || { echo "no $key; generate one with ssh-keygen -t ed25519" >&2; exit 2; }

ssh-copy-id -i "$key" "$target" >/dev/null 2>&1 || true
ssh -o BatchMode=yes "$target" true || { echo "key login to $target failed" >&2; exit 1; }

ssh "$target" "sudo -n true" 2>/dev/null || {
    echo "sudo needs a password on the Pi; scripts/pi-deploy-plc.sh needs passwordless sudo." >&2
    echo "Once, on the Pi:  echo '${target%@*} ALL=(ALL) NOPASSWD: ALL' | sudo tee /etc/sudoers.d/010_${target%@*}-nopasswd" >&2
    exit 1
}

ssh "$target" bash -s "$hostname" <<'REMOTE'
set -euo pipefail
hostname="$1"
arch="$(uname -m)"; [ "$arch" = aarch64 ] || { echo "the box build is aarch64, got $arch" >&2; exit 1; }
. /etc/os-release; echo "os: $PRETTY_NAME  kernel: $(uname -r)  model: $(tr -d '\0' </proc/device-tree/model)"
if [ "$(hostname)" != "$hostname" ]; then
    sudo hostnamectl set-hostname "$hostname"
    sudo sed -i "s/127\.0\.1\.1.*/127.0.1.1\t$hostname/" /etc/hosts
fi
sudo apt-get update -qq && sudo apt-get -y -qq upgrade
sudo apt-get -y -qq install avahi-daemon
# Key-only SSH from now on.
sudo install -m 644 /dev/stdin /etc/ssh/sshd_config.d/10-key-only.conf <<CONF
PasswordAuthentication no
KbdInteractiveAuthentication no
CONF
sudo systemctl reload ssh
echo "ip: $(hostname -I)"
echo "ports 44818/2222 free: $(ss -lun | grep -cE ':(44818|2222) ' | sed 's/^0$/yes/;s/^[1-9].*/NO/')"
REMOTE

cat <<EOT

Pi is ready. Next, from this checkout:
  PLC_API=0.0.0.0:8080 scripts/pi-deploy-plc.sh ${target%@*}@$(ssh "$target" hostname -I | awk '{print $1}')
EOT
