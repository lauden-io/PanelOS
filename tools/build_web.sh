#!/bin/bash
# Builds the PanelOS web bundle (buildroot/configs/panelos_web_defconfig):
# a SquashFS root file system with Mesa (Panfrost), WPE WebKit and the
# panelos-web helper. The first build takes hours (LLVM, WebKit).
#
#   BRSRC   Buildroot source  (default /root/nspanel/src/buildroot)
#   BROUT   output directory  (default /root/nspanel/br-web)
#   BREXT   copy of the external tree (default /root/nspanel/br2-external-web)
#
# Result: $BROUT/images/rootfs.squashfs
set -euo pipefail
REPO="${REPO:-$(cd "$(dirname "$0")/.." && pwd)}"
BRSRC="${BRSRC:-/root/nspanel/src/buildroot}"
BROUT="${BROUT:-/root/nspanel/br-web}"
BREXT="${BREXT:-/root/nspanel/br2-external-web}"

rm -rf "$BREXT"
cp -r "$REPO/buildroot" "$BREXT"
find "$BREXT" -type f -exec grep -Il . {} + | while IFS= read -r f; do sed -i 's/\r$//' "$f"; done
find "$BREXT" -type f -exec chmod 644 {} +
find "$BREXT" -name '*.sh' -exec chmod 755 {} +

make -C "$BRSRC" O="$BROUT" BR2_EXTERNAL="$BREXT" panelos_web_defconfig >/dev/null
# The panelos-web helper is a local package: Buildroot copies its sources only
# for its first build. Without its stamps it is copied and built again, so a
# changed helper gets into the bundle.
make -C "$BRSRC" O="$BROUT" panelos-web-clean-for-rebuild >/dev/null
make -C "$BRSRC" O="$BROUT" "$@" 2>&1 | tail -20
ls -la "$BROUT/images/rootfs.squashfs"
