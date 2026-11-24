# Installing Helmsman on a Linux machine

A release is one archive per processor: the box as a static binary (no
libraries needed from the machine), the browser editor it serves, and its
systemd unit. `scripts/install.sh` installs it; nothing is compiled on the
machine and no container runs.

| Archive | For |
|---|---|
| `helmsman-x86_64-unknown-linux-musl.tar.gz` | 64-bit Intel and AMD |
| `helmsman-aarch64-unknown-linux-musl.tar.gz` | 64-bit ARM |

Each has a `.sha256` beside it. The machine needs systemd, `curl` and
`tar`.

## Install

As root, with the release's tag:

    curl -fsSL https://raw.githubusercontent.com/JimothyJohn/helmsman/master/scripts/install.sh \
      | sudo bash -s -- v0.1.0

The installer downloads the archive for the machine's processor and its
checksum, and installs nothing unless the checksum matches and the archive
holds exactly a release (the binary, the unit, the editor's files; no
other file, no path outside it, no link). Then it:

- puts the binary at `/usr/local/bin/helmsman-plc` and the editor under
  `/usr/local/share/helmsman-plc/ui`;
- writes `/etc/helmsman-plc/env`, the service's settings, **if there is
  none**;
- installs `helmsman-plc.service`, enables it and starts it.

The editor is then at `http://127.0.0.1:8080/` on that machine. To reach
it from the network instead, install with `--lan` (or set
`PLC_API=0.0.0.0:8080` in `/etc/helmsman-plc/env` and restart the
service). From the network the box serves HTTPS with a certificate it
makes itself; its fingerprint is in the journal. The installer opens no
firewall port: 8080/tcp for the editor, and 44818/tcp and 2222/udp for
EtherNet/IP, are yours to open to the networks that should reach them.

Make the first login, and read the box's token:

    sudo helmsman-plc account add <name> engineer
    journalctl -u helmsman-plc | grep 'API token'

## Without a network

Copy the archive and its `.sha256` to the machine, with `install.sh`:

    sudo ./install.sh --local helmsman-x86_64-unknown-linux-musl.tar.gz

## Upgrade

Run the installer again with the newer tag. The binary, the editor and the
unit are replaced and the service restarted. `/etc/helmsman-plc/env`, the
relay token, and everything under `/var/lib/helmsman-plc` (projects, their
history, accounts, the store of what was deployed) are not touched.

There is no automatic update: a machine runs the release it was given
until the installer is run again.

## Remove

    sudo systemctl disable --now helmsman-plc
    sudo rm /etc/systemd/system/helmsman-plc.service /usr/local/bin/helmsman-plc
    sudo rm -r /usr/local/share/helmsman-plc /etc/helmsman-plc
    sudo systemctl daemon-reload

The projects stay in `/var/lib/helmsman-plc` (under `/var/lib/private` on
a current systemd) until you remove that too.

## Making a release (maintainers)

`.github/workflows/release.yml` builds both archives on a `v*` tag and
attaches them to a GitHub release. The tag must be the box's version:
`v0.1.0` for `helmsman-plc` 0.1.0 in `crates/helmsman-plc/Cargo.toml`, or
the workflow stops before building.

    git tag -a v0.1.0 <commit on master> -m v0.1.0
    git push origin v0.1.0

A pull request that touches the workflow, the installer, the packer or the
unit runs the same build, packs both archives and installs each into a
test root, and publishes nothing.

To try an archive of your own build:

    make canvas-web
    scripts/package-release.sh x86_64-unknown-linux-musl PATH/TO/helmsman-plc target/canvas-dist target/dist
    sudo scripts/install.sh --local target/dist/helmsman-x86_64-unknown-linux-musl.tar.gz

## Not in a release yet

- The Skipper's own image, macOS and Windows.
- Signatures: the checksum shows the download is whole, not who made it.
- Automatic updates.
