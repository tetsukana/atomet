#!/bin/bash
set -e

WORKSPACE=${GITHUB_WORKSPACE:-/src}
OUT_DIR=$WORKSPACE/output
mkdir -p $OUT_DIR

# Buildroot passes BINARIES_DIR as $1 (images directory)
IMAGES_DIR="${1:-${BINARIES_DIR}}"
cd "$IMAGES_DIR"
cp -dpf uImage.lzma factory_t31_ZMC6tiIDQN
mv rootfs.squashfs rootfs_hack.squashfs
cp -f factory_t31_ZMC6tiIDQN rootfs_hack.squashfs $OUT_DIR

echo "=== Build artifacts ==="
ls -lh $OUT_DIR/
