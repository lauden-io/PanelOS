#!/usr/bin/env bash
# Build the full development image for the recovery slot:
# mainline kernel (kernel/) + Buildroot root filesystem (buildroot/) as initramfs.
# Runs in WSL/Linux as root. Output: $OUT/recovery-dev.img (+ md5), also copied
# to /tmp/nspanel_out.
#
# Environment (defaults):
#   REPO   this repository               KSRC  mainline kernel source tree
#   BRSRC  Buildroot source tree         OUT   output directory
#   JOBS   parallel jobs                 NSPANEL_SSH_PUBKEY_FILE  key for root SSH login
#   STOCK_RESOURCE  the stock resource partition from a panel backup (default:
#                   the first /root/nspanel-backups/*/resource.img)
#   KERNEL_PACK     lz4 (default: packed kernel, the chain loader unpacks it;
#                   keeps the image small for the 32 MiB boot partition) or none
set -euo pipefail

# WSL appends the Windows PATH (with spaces), which Buildroot refuses.
export PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin

REPO="${REPO:-$(cd "$(dirname "$0")/.." && pwd)}"
KSRC="${KSRC:-/root/nspanel/src/linux}"
BRSRC="${BRSRC:-/root/nspanel/src/buildroot}"
OUT="${OUT:-/root/nspanel/out}"
BROUT="${BROUT:-/root/nspanel/br-out}"
BREXT="${BREXT:-/root/nspanel/br2-external}"
JOBS="${JOBS:-$(nproc)}"
export NSPANEL_SSH_PUBKEY_FILE="${NSPANEL_SSH_PUBKEY_FILE:-/root/.ssh/nspanel_dev.pub}"
DTB_NAME=px30-sonoff-nspanel-pro-86
# Backlight from the bootloader logo on (the boot animation and S20backlight
# use the same value).
BOOT_BRIGHTNESS=180
KERNEL_PACK="${KERNEL_PACK:-lz4}"

STOCK_RESOURCE="${STOCK_RESOURCE:-$(ls /root/nspanel-backups/*/resource.img 2>/dev/null | head -1 || true)}"
if [ -z "$STOCK_RESOURCE" ] || [ ! -f "$STOCK_RESOURCE" ]; then
  echo "ERROR: no stock resource partition found. Back up a panel first (install guide,"
  echo "step 6) or set STOCK_RESOURCE=/path/to/resource.img. The stock U-Boot needs its own"
  echo "device tree from it: for the boot logo and for loading the image in about 2 s."
  exit 1
fi
echo "== stock resource partition: $STOCK_RESOURCE"

mkdir -p "$OUT" "$BROUT"

if [ ! -f "$NSPANEL_SSH_PUBKEY_FILE" ]; then
  echo "== generating a development SSH key (outside the repo)"
  mkdir -p "$(dirname "$NSPANEL_SSH_PUBKEY_FILE")"
  ssh-keygen -q -t ed25519 -N "" -C nspanel-dev -f "${NSPANEL_SSH_PUBKEY_FILE%.pub}"
fi

########## kernel ##########
(
  export ARCH=arm64 CROSS_COMPILE=aarch64-linux-gnu-
  cd "$KSRC"
  echo "== kernel: patches"
  for p in "$REPO"/kernel/patches/*.patch; do
    if git apply --check -R "$p" 2>/dev/null; then
      echo "already applied: $(basename "$p")"
    else
      git apply "$p"
      echo "applied: $(basename "$p")"
    fi
  done
  cp "$REPO/kernel/dts/$DTB_NAME.dts" arch/arm64/boot/dts/rockchip/
  grep -q "$DTB_NAME.dtb" arch/arm64/boot/dts/rockchip/Makefile ||
    echo "dtb-\$(CONFIG_ARCH_ROCKCHIP) += $DTB_NAME.dtb" >> arch/arm64/boot/dts/rockchip/Makefile
  echo "== kernel: config"
  make defconfig >/dev/null
  cp "$REPO/kernel/configs/nspanel.config" "$OUT/nspanel.config"  # merge_config.sh cannot handle spaces
  scripts/kconfig/merge_config.sh -m .config "$OUT/nspanel.config" >/dev/null
  make olddefconfig >/dev/null
  missing=0
  for o in DRM_ROCKCHIP ROCKCHIP_VOP ROCKCHIP_DW_MIPI_DSI PHY_ROCKCHIP_INNO_DSIDPHY \
           DRM_PANEL_SITRONIX_ST7701 DRM_FBDEV_EMULATION FRAMEBUFFER_CONSOLE BACKLIGHT_PWM \
           PWM_ROCKCHIP I2C_RK3X MFD_RK8XX_I2C REGULATOR_RK808 MMC_DW_ROCKCHIP PSTORE_RAM \
           PSTORE_CONSOLE ROCKCHIP_THERMAL SYSCON_REBOOT_MODE BLK_DEV_INITRD \
           CFG80211 MAC80211 RTW88_8723DS PWRSEQ_SIMPLE RESET_GPIO EXT4_FS \
           TOUCHSCREEN_HYNITRON_CSTXXX INPUT_EVDEV I2C_CHARDEV IIO STK3310 \
           SND SND_SOC SND_SOC_ROCKCHIP_I2S SND_SOC_RK817 SND_SIMPLE_CARD \
           SND_SOC_ROCKCHIP_PDM SND_SOC_DMIC DW_WATCHDOG WATCHDOG_HANDLE_BOOT_ENABLED \
           DRM_PANFROST PM_DEVFREQ SQUASHFS SQUASHFS_ZSTD BLK_DEV_LOOP RD_XZ; do
    grep -q "^CONFIG_$o=y" .config || { echo "NOT BUILT IN: CONFIG_$o"; missing=1; }
  done
  [ "$missing" -eq 0 ] || exit 1
  echo "== kernel: build"
  make -j"$JOBS" Image "rockchip/$DTB_NAME.dtb" 2>&1 | grep -E "error|Error" || true
  ls -la arch/arm64/boot/Image "arch/arm64/boot/dts/rockchip/$DTB_NAME.dtb"
  # The stock U-Boot gets the stock device tree and the boot logo (resource
  # image in the boot image's second stage); the chain loader in front of the
  # kernel hands the kernel its own device tree (tools/chainload).
  bash "$REPO/tools/make_uboot_dtb.sh" "$STOCK_RESOURCE" scripts/dtc/dtc "$BOOT_BRIGHTNESS" "$OUT/uboot.dtb"
  python3 "$REPO/tools/pack_rk_resource.py" "$OUT/resource.img" \
    "rk-kernel.dtb=$OUT/uboot.dtb" "logo.bmp=$REPO/assets/bootlogo.bmp"
  bash "$REPO/tools/chainload/build.sh" "$KSRC" "$OUT/chain.bin"
  # Packed (LZ4) by default: about 1 s to unpack (caches on), but the stock
  # U-Boot loads the 10 MB smaller image that much faster; same boot time.
  pack=""
  [ "$KERNEL_PACK" = lz4 ] && pack=--lz4
  python3 "$REPO/tools/arm64_chain_pack.py" "$OUT/chain.bin" "arch/arm64/boot/dts/rockchip/$DTB_NAME.dtb" \
    arch/arm64/boot/Image "$OUT/kernel.bin" $pack
)

########## root filesystem ##########
echo "== rootfs: Buildroot"
rm -rf "$BREXT"
cp -r "$REPO/buildroot" "$BREXT"
# Windows line ends out of the text files only (grep -I skips binaries): the
# same step on every file removed the 0D of each 0D 0A pair in splash.bin and
# broke the boot animation.
find "$BREXT" -type f -exec grep -Il . {} + | while IFS= read -r f; do sed -i 's/\r$//' "$f"; done
find "$BREXT" -type f -exec chmod 644 {} +
chmod 755 "$BREXT/board/nspanel/post-build.sh" "$BREXT"/board/nspanel/rootfs_overlay/etc/init.d/* \
  "$BREXT"/board/nspanel/rootfs_overlay/usr/sbin/*
# Small helper tools (board/nspanel/src/*.c), compiled static into the overlay
mkdir -p "$BREXT/board/nspanel/rootfs_overlay/usr/sbin"
for c in "$BREXT"/board/nspanel/src/*.c; do
  bin="$BREXT/board/nspanel/rootfs_overlay/usr/sbin/$(basename "$c" .c)"
  aarch64-linux-gnu-gcc -static -Os -Wall -o "$bin" "$c" -lm
  aarch64-linux-gnu-strip "$bin"
  chmod 755 "$bin"
done
make -C "$BRSRC" O="$BROUT" BR2_EXTERNAL="$BREXT" nspanel_pro_86_defconfig >/dev/null
# Buildroot does not rebuild a package when its options change: an older
# output directory still has wpa_supplicant without syslog support.
if grep -qs '^#CONFIG_DEBUG_SYSLOG=y' "$BROUT"/build/wpa_supplicant-*/wpa_supplicant/.config; then
  make -C "$BRSRC" O="$BROUT" BR2_EXTERNAL="$BREXT" wpa_supplicant-dirclean >/dev/null
fi
# Panel app (Rust, app/): needs Buildroot's host Rust and the target
# libraries it links (fontconfig, TensorFlow Lite, WebRTC audio processing)
# first; the binary then goes into the overlay.
echo "== panel app"
make -C "$BRSRC" O="$BROUT" -j"$JOBS" host-rustc fontconfig tensorflow-lite webrtc-audio-processing 2>&1 | tail -2
BROUT="$BROUT" bash "$REPO/tools/build_app.sh" 2>&1 | tail -2
install -D -m 755 /root/nspanel/app-build/target/aarch64-unknown-linux-gnu/release/nspanel-app \
  "$BREXT/board/nspanel/rootfs_overlay/usr/bin/nspanel-app"
# Wake word models (microWakeWord), cached outside the repo, not committed.
# The public models of ESPHome voice satellites, all under a licence
# (esphome/micro-wake-word-models and OHF-Voice/micro-wake-word: Apache-2.0).
# "stop" is internal (stops an answer or a ringing timer), not offered as a
# wake word. Models without a stated licence are not shipped.
WW_CACHE="${WW_CACHE:-/root/nspanel/wakeword-cache}"
# A fixed commit of the model repository; every file is checked against
# tools/wakewords.sha256.
WW_ESPHOME=https://raw.githubusercontent.com/esphome/micro-wake-word-models/40ff33f57f8fc6ad71a75ef085abbf742495b225/models/v2
WW_OHF=https://github.com/OHF-Voice/micro-wake-word/releases/download
WW_DIR="$BREXT/board/nspanel/rootfs_overlay/usr/share/nspanel/wakewords"
mkdir -p "$WW_DIR"
# Buildroot never removes old overlay files from the target: start the
# model folder afresh, so a model dropped from this list is gone.
rm -rf "$BROUT/target/usr/share/nspanel/wakewords"
for entry in   "okay_nabu $WW_OHF/okay_nabu_20241226.3"   "stop $WW_OHF/stop"   "hey_jarvis $WW_ESPHOME"   "hey_mycroft $WW_ESPHOME"   "alexa $WW_ESPHOME"   "hey_home_assistant $WW_ESPHOME/experiments"   "okay_computer $WW_ESPHOME/experiments"   "choo_choo_homie $WW_ESPHOME/experiments"   "hey_peppa_pig $WW_ESPHOME/experiments"; do
  set -- $entry
  m=$1; base=$2
  cache="$WW_CACHE/$(echo "$base" | md5sum | cut -c1-8)"
  mkdir -p "$cache"
  for ext in tflite json; do
    [ -s "$cache/$m.$ext" ] || curl -fsSL -o "$cache/$m.$ext" "$base/$m.$ext"
    sum=$(awk -v f="$m.$ext" '$2 == f {print $1}' "$REPO/tools/wakewords.sha256")
    if [ -z "$sum" ] || ! echo "$sum  $cache/$m.$ext" | sha256sum -c --quiet; then
      echo "wake word model $m.$ext: checksum missing or wrong (tools/wakewords.sha256)" >&2
      exit 1
    fi
    install -m 644 "$cache/$m.$ext" "$WW_DIR/"
  done
done
make -C "$BRSRC" O="$BROUT" -j"$JOBS" 2>&1 | tail -5
ROOTFS="$BROUT/images/rootfs.cpio.xz"
ls -la "$ROOTFS"

########## boot image ##########
echo "== boot image"
# Kernel messages only on the serial console (and pstore), not on the display:
# the boot splash owns the screen; no blinking cursor on the empty console.
CMDLINE="earlycon=uart8250,mmio32,0xff160000 console=ttyS2,1500000n8 vt.global_cursor_default=0 loglevel=7 panic=10 clk_ignore_unused pd_ignore_unused regulator_ignore_unused rdinit=/init"
python3 "$REPO/tools/mkbootimg_v0.py" --kernel "$OUT/kernel.bin" --ramdisk "$ROOTFS" \
  --second "$OUT/resource.img" --cmdline "$CMDLINE" -o "$OUT/recovery-dev.img"
size=$(stat -c %s "$OUT/recovery-dev.img")
limit=$((131072 * 512))
echo "recovery-dev.img: $size bytes (recovery partition: $limit; boot partition: $((65536 * 512)))"
[ "$size" -le "$limit" ] || { echo "ERROR: image does not fit the recovery partition"; exit 1; }
# Without the stock Android the boot partition holds a second copy (tools/remove_android.sh), and the A/B update
# keeps an image in each slot: those need an image that fits the boot partition. A bigger one still boots from
# recovery; tools/remove_android.sh then refuses the second copy.
boot=$((65536 * 512))
if [ "$size" -le "$boot" ]; then
  echo "fits the boot partition: $((boot - size)) bytes left"
else
  echo "WARNING: $((size - boot)) bytes too big for the boot partition: recovery only, no second copy"
fi
(cd "$OUT" && md5sum recovery-dev.img > recovery-dev.img.md5 && cat recovery-dev.img.md5)
mkdir -p /tmp/nspanel_out
cp "$OUT/recovery-dev.img" "$OUT/recovery-dev.img.md5" /tmp/nspanel_out/
chmod -R a+rX /tmp/nspanel_out
echo "done: /tmp/nspanel_out/recovery-dev.img"
