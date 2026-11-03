#!/bin/bash -e
# pi-gen substage: install Helmsman into the image's root the way
# scripts/pi-deploy-plc.sh installs it over SSH. `files/` is filled by
# scripts/pi-image.sh before the build: the arm64 binary, the canvas, the
# seed program, the units, the environment file and the image manifest.
files="${BASH_SOURCE%/*}/files"

install -m 755 "${files}/helmsman-plc" "${ROOTFS_DIR}/usr/local/bin/helmsman-plc"
install -d -m 755 "${ROOTFS_DIR}/etc/helmsman-plc" "${ROOTFS_DIR}/etc/helmsman-plc/seed"
install -m 644 "${files}"/seed/*.st "${ROOTFS_DIR}/etc/helmsman-plc/seed/"
install -m 644 "${files}/env" "${ROOTFS_DIR}/etc/helmsman-plc/env"
install -m 644 "${files}/image-manifest" "${ROOTFS_DIR}/etc/helmsman-plc/image-manifest"
# Empty until first boot pairs the box; the unit loads it as a credential.
install -m 600 /dev/null "${ROOTFS_DIR}/etc/helmsman-plc/relay-token"

# The canvas, read-only for the service.
rm -rf "${ROOTFS_DIR}/usr/local/share/helmsman-plc/ui"
install -d -m 755 "${ROOTFS_DIR}/usr/local/share/helmsman-plc/ui"
cp -R "${files}/ui/." "${ROOTFS_DIR}/usr/local/share/helmsman-plc/ui/"
chown -R root:root "${ROOTFS_DIR}/usr/local/share/helmsman-plc"
chmod -R u=rwX,go=rX "${ROOTFS_DIR}/usr/local/share/helmsman-plc"

install -m 644 "${files}/helmsman-plc.service" "${ROOTFS_DIR}/etc/systemd/system/helmsman-plc.service"
install -m 644 "${files}/helmsman-firstboot.service" "${ROOTFS_DIR}/etc/systemd/system/helmsman-firstboot.service"
install -m 755 "${files}/helmsman-firstboot.sh" "${ROOTFS_DIR}/usr/local/sbin/helmsman-firstboot"

# Where the pairing files go on the boot partition (docs/BOX.md).
install -d -m 755 "${ROOTFS_DIR}/boot/firmware/helmsman"
cat > "${ROOTFS_DIR}/boot/firmware/helmsman/README.txt" <<'EOF'
Helmsman box: pairing files, read once at first boot by helmsman-firstboot.

  relay-url     the relay's https URL, one line (optional)
  relay-token   the box's relay token (hmr1_...), one line (optional)
  box-name      this box's name on the site (optional; default: the hostname)

The token file is moved into the box's state directory and removed from
here. Without these files the box runs without an agent; add them later
and run: sudo systemctl start helmsman-firstboot
EOF

on_chroot <<'EOF'
systemctl enable helmsman-firstboot.service
systemctl enable helmsman-plc.service
EOF
