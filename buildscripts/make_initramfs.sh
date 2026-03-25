#!/bin/bash
set -o errexit
set -o errtrace
set -o nounset
set -o pipefail

echo "=== build initramfs ==="

WORKSPACE=${GITHUB_WORKSPACE:-/src}
BASE_DIR=${BUILDROOT_BASE_DIR:-$WORKSPACE/build/buildroot-2024.02}
ROOTFS_DIR=$BASE_DIR/output/initramfs_root
OUT_DIR=$BASE_DIR/output

[ -f $BASE_DIR/staging/bin-init/fsck.fat ]   || make dosfstools-init
[ -f $BASE_DIR/staging/bin-init/fsck.exfat ] || make exfatprogs-init
[ -f $BASE_DIR/staging/bin-init/busybox ]    || make busybox-init
[ -f $BASE_DIR/host/usr/bin/mkimage ]        || make host-uboot-tools

rm -rf $ROOTFS_DIR
mkdir -p $ROOTFS_DIR

cd $ROOTFS_DIR
mkdir -p {bin,dev,etc,lib,mnt,proc,root,sbin,sys,tmp}

cp -pR "$WORKSPACE"/initramfs_skeleton/* $ROOTFS_DIR
cp $OUT_DIR/build/dosfstools-init-3.0.28/fsck.fat     $ROOTFS_DIR/bin/
cp $OUT_DIR/build/exfatprogs-init-1.2.2/fsck/fsck.exfat $ROOTFS_DIR/bin/
cp $OUT_DIR/build/busybox-init-1.24.1/busybox         $ROOTFS_DIR/bin/

rm -f $ROOTFS_DIR/README.md

# Windows の Git は symlink をテキストファイルとして保存するため、
# busybox applet の symlink をここで明示的に作成する
for applet in ash blkid cat chgrp chmod chown chroot cp echo grep hexdump \
              ls mkdir mkfifo mknod mount mv pwd rm rmdir sed sh sleep \
              switch_root sync touch unzip; do
    ln -sf busybox "$ROOTFS_DIR/bin/$applet"
done
chmod +x "$ROOTFS_DIR/bin/busybox" \
         "$ROOTFS_DIR/bin/fsck.fat" \
         "$ROOTFS_DIR/bin/fsck.exfat" \
         "$ROOTFS_DIR/init"

sudo mknod $ROOTFS_DIR/dev/console c 5 1
sudo mknod $ROOTFS_DIR/dev/null    c 1 3
sudo mknod $ROOTFS_DIR/dev/tty0    c 4 0
sudo mknod $ROOTFS_DIR/dev/tty1    c 4 1

find . | cpio -H newc -o > $OUT_DIR/images/initramfs.cpio
