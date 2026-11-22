#!/usr/bin/env bash
# Build the UI bundle: the panel app for the GPU together with the files its
# pages read at run time (pictures, fonts, sounds, layout data), as one
# SquashFS image. The panel mounts it at /opt/panelos-ui (S51panelos-ui):
#
#   bin/nspanel-app   the GPU app (GPU=1 tools/build_app.sh)
#   share/ui/         app/ui as the app was built with it, without the .slint
#                     files (they are compiled into the app). The app opens
#                     these files by their absolute path under
#                     /opt/panelos-ui/share/ui (PANELOS_UI_DIR, tools/build_app.sh).
#
# Run as root in Linux/WSL after tools/build_image.sh (host Rust and the
# image's libraries) and tools/build_web.sh (the GPU app links the web
# bundle's libgbm; its host tools bring mksquashfs).
#
# Usage: tools/build_ui_bundle.sh           builds the GPU app, then the bundle
#        BUILD=0 tools/build_ui_bundle.sh   packs the last GPU build only
# Env:   BRWEB  web bundle's Buildroot output (default /root/nspanel/br-web)
#        WORK   GPU app build folder          (default /root/nspanel/app-build-gpu)
#        OUT    output folder                 (default /root/nspanel/out)
# Result: $OUT/panelos-ui.squashfs (+ .md5); tools/install_bundles.sh puts it on a panel.
set -euo pipefail

REPO="${REPO:-$(cd "$(dirname "$0")/.." && pwd)}"
BRWEB="${BRWEB:-/root/nspanel/br-web}"
WORK="${WORK:-/root/nspanel/app-build-gpu}"
OUT="${OUT:-/root/nspanel/out}"
BUILD="${BUILD:-1}"
# Where the app finds its files on the panel; compiled into the app.
UI_DIR=/opt/panelos-ui/share/ui

die() { echo "ERROR: $*" >&2; exit 1; }

if [ "$BUILD" = 1 ]; then
  GPU=1 BRWEB="$BRWEB" WORK="$WORK" PANELOS_UI_DIR="$UI_DIR" bash "$REPO/tools/build_app.sh"
fi

BIN="$WORK/target/aarch64-unknown-linux-gnu/release/nspanel-app"
SRC="$WORK/src/ui"
[ -x "$BIN" ] && [ -f "$SRC/app.slint" ] || die "no GPU app build in $WORK (run without BUILD=0)"
# build_app.sh copies the sources first, then builds: an app older than its
# sources means the last build did not finish.
[ "$BIN" -nt "$SRC/app.slint" ] || die "the app in $WORK is older than its sources: the last build did not finish"
[[ $(file -b "$BIN") == *"ARM aarch64"* ]] || die "$BIN is not an aarch64 program"
[[ $(readelf -d "$BIN") == *"/opt/panelos-web/usr/lib"* ]] || die "$BIN is not the GPU build (GPU=1 tools/build_app.sh)"
grep -q -a -F "$UI_DIR/" "$BIN" || die "$BIN looks for its files somewhere else than $UI_DIR (PANELOS_UI_DIR); build it again"

MKSQUASHFS="$BRWEB/host/bin/mksquashfs"
[ -x "$MKSQUASHFS" ] || MKSQUASHFS=$(command -v mksquashfs || true)
[ -n "$MKSQUASHFS" ] || die "mksquashfs not found: build the web bundle first (tools/build_web.sh builds it) or install squashfs-tools"

STAGE="$OUT/panelos-ui-root"
IMG="$OUT/panelos-ui.squashfs"
rm -rf "$STAGE"
mkdir -p "$STAGE/bin" "$STAGE/share/ui"
install -m 755 "$BIN" "$STAGE/bin/nspanel-app"
rsync -a --exclude='*.slint' "$SRC/" "$STAGE/share/ui/"
find "$STAGE/share" -type d -exec chmod 755 {} +
find "$STAGE/share" -type f -exec chmod 644 {} +

# zstd and no extended attributes: what the panel's kernel reads.
rm -f "$IMG.tmp"
"$MKSQUASHFS" "$STAGE" "$IMG.tmp" -comp zstd -b 131072 -all-root -no-xattrs -noappend -no-progress >/dev/null
mv -f "$IMG.tmp" "$IMG"
files=$(find "$STAGE" -type f | wc -l)
rm -rf "$STAGE"
(cd "$OUT" && md5sum panelos-ui.squashfs > panelos-ui.squashfs.md5)
echo "UI bundle: $(stat -c %s "$IMG") bytes, $files files, md5 $(cut -d' ' -f1 "$IMG.md5")"
echo "done: $IMG"
