#!/bin/bash -e
# pi-gen stage: start from stage2's root (Raspberry Pi OS Lite).
if [ ! -d "${ROOTFS_DIR}" ]; then
	copy_previous
fi
