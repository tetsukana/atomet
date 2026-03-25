#!/bin/bash
# Web (Svelte) のみビルド (docker compose run --rm builder web_build)
set -e

WORKSPACE=/src
OUTPUT="$WORKSPACE/output/web"

echo "=== Building web (Svelte) ==="

# /src はWindowsホストマウントのためシンボリックリンク不可。
# /tmp 上にコピーしてビルドする。
BUILD_TMP=/tmp/web_build
rm -rf "$BUILD_TMP"
cp -r "$WORKSPACE/web" "$BUILD_TMP"

cd "$BUILD_TMP"
npm ci
npm run build

rm -rf "$OUTPUT"
mkdir -p "$OUTPUT"
cp -r "$BUILD_TMP/build/." "$OUTPUT/"
rm -rf "$BUILD_TMP"

echo "=== Done: $OUTPUT ==="
echo "Deploy: scp -r output/web/* root@atomet.local:/media/mmc/web/"
