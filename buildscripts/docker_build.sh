#!/bin/bash
# Docker コンテナ内で実行されるビルドスクリプト
# docker-compose run builder  または  docker exec -it <container> docker_build
set -e

WORKSPACE=/src
BUILDROOT_DIR=/build/buildroot-2024.02

echo "=== atomet Docker build ==="
echo "WORKSPACE: $WORKSPACE"
echo "BUILDROOT_DIR: $BUILDROOT_DIR"

# named volume 初回起動時: イメージ内の Buildroot をコピー
if [ ! -f "$BUILDROOT_DIR/Makefile" ]; then
    echo "=== Initializing Buildroot in named volume ==="
    mkdir -p "$BUILDROOT_DIR"
    cp -a /buildroot-dist/buildroot-2024.02/. "$BUILDROOT_DIR/"
fi

mkdir -p "$BUILDROOT_DIR/output"

# Windows bind mount との時刻ずれ対策: output/ 内の「未来」タイムスタンプのファイルを現在時刻に修正
fix_future_timestamps() {
    local ref
    ref=$(mktemp)
    find "$BUILDROOT_DIR/output" -newer "$ref" -exec touch {} + 2>/dev/null || true
    rm -f "$ref"
}

# カスタムパッケージを Buildroot にコピー
cp -pR "$WORKSPACE/custompackages/package/"* "$BUILDROOT_DIR/package/"

# カーネル設定に絶対パスを追加
cp "$WORKSPACE/configs/kernel.config" /tmp/kernel_build.config
echo "CONFIG_CROSS_COMPILE=\"$BUILDROOT_DIR/output/host/usr/bin/mipsel-ingenic-linux-gnu-\"" >> /tmp/kernel_build.config
echo "CONFIG_INITRAMFS_SOURCE=\"$BUILDROOT_DIR/output/images/initramfs.cpio\"" >> /tmp/kernel_build.config

# kernel.config の一時コピーを参照するよう defconfig を上書き
cp "$WORKSPACE/configs/atomet_defconfig" /tmp/build_defconfig
sed -i 's|BR2_LINUX_KERNEL_CUSTOM_CONFIG_FILE=.*|BR2_LINUX_KERNEL_CUSTOM_CONFIG_FILE="/tmp/kernel_build.config"|' /tmp/build_defconfig
# 相対パスを絶対パスに変換
sed -i "s|BR2_LINUX_KERNEL_PATCH=\"\.\./|BR2_LINUX_KERNEL_PATCH=\"$WORKSPACE/|" /tmp/build_defconfig
sed -i "s|BR2_ROOTFS_OVERLAY=\"\.\./|BR2_ROOTFS_OVERLAY=\"$WORKSPACE/|" /tmp/build_defconfig
sed -i "s|BR2_ROOTFS_POST_BUILD_SCRIPT=\"\.\./|BR2_ROOTFS_POST_BUILD_SCRIPT=\"$WORKSPACE/|" /tmp/build_defconfig
sed -i "s|BR2_ROOTFS_POST_IMAGE_SCRIPT=\"\.\./|BR2_ROOTFS_POST_IMAGE_SCRIPT=\"$WORKSPACE/|" /tmp/build_defconfig

cd "$BUILDROOT_DIR"

# --- defconfig 変更チェック ---
# make defconfig は .config のタイムスタンプを更新するため、変更がない場合はスキップする。
# スキップしないと .config が常に最新となり Buildroot がカーネルを毎回再ビルドしてしまう。
DEFCONFIG_STAMP="$BUILDROOT_DIR/output/.defconfig.stamp"
DEFCONFIG_HASH=$(md5sum /tmp/build_defconfig | cut -d' ' -f1)
SAVED_DEFCONFIG_HASH=$(cat "$DEFCONFIG_STAMP" 2>/dev/null || echo "")

if [ "$DEFCONFIG_HASH" != "$SAVED_DEFCONFIG_HASH" ] || [ ! -f "$BUILDROOT_DIR/.config" ]; then
    echo "=== defconfig changed: applying ==="
    make defconfig BR2_DEFCONFIG=/tmp/build_defconfig
    make olddefconfig
    echo "$DEFCONFIG_HASH" > "$DEFCONFIG_STAMP"
else
    echo "=== defconfig unchanged: skipping make defconfig ==="
fi
fix_future_timestamps

# --- カーネル再ビルド判定 ---
# initramfs_skeleton/make_initramfs.sh または kernel.config が変わった場合のみ再ビルド
INITRAMFS_CPIO="$BUILDROOT_DIR/output/images/initramfs.cpio"
KERNEL_REBUILD_STAMP="$BUILDROOT_DIR/output/.kernel_rebuild.stamp"

INITRAMFS_HASH=$(find "$WORKSPACE/initramfs_skeleton" "$WORKSPACE/buildscripts/make_initramfs.sh" \
    -type f | sort | xargs md5sum | md5sum | cut -d' ' -f1)
KERNEL_CONFIG_HASH=$(md5sum /tmp/kernel_build.config | cut -d' ' -f1)
CURRENT_KERNEL_HASH="${INITRAMFS_HASH}_${KERNEL_CONFIG_HASH}"
SAVED_KERNEL_HASH=$(cat "$KERNEL_REBUILD_STAMP" 2>/dev/null || echo "")

if [ "$CURRENT_KERNEL_HASH" != "$SAVED_KERNEL_HASH" ] || [ ! -f "$INITRAMFS_CPIO" ]; then
    echo "=== initramfs/kernel config changed: rebuilding initramfs + kernel ==="
    GITHUB_WORKSPACE=$WORKSPACE BUILDROOT_BASE_DIR=$BUILDROOT_DIR "$WORKSPACE/buildscripts/make_initramfs.sh"
    fix_future_timestamps
    make rootfs-initramfs
    fix_future_timestamps
    make linux-rebuild
    echo "$CURRENT_KERNEL_HASH" > "$KERNEL_REBUILD_STAMP"
else
    echo "=== initramfs/kernel config unchanged: skipping kernel rebuild ==="
fi

# rootfs (squashfs) + 全体ビルド
fix_future_timestamps
make

# 成果物は post_image.sh が $WORKSPACE/output/ にコピー済み
echo "=== Build complete ==="
ls -lh "$WORKSPACE/output/"
