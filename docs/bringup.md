# Bring-up notes (Sonoff NSPanel Pro 86)

How the mainline system was brought up on the panel, with the facts and pitfalls found on the way.

## Stock boot images

| Partition | Format | Content |
|---|---|---|
| `kernel` | `KRNL` wrapper | arm64 kernel Image (19.7 MB) |
| `boot` | Android boot image v0, page size 2048 | kernel + 1.4 MB ramdisk + "second" (1 MB) |
| `recovery` | Android boot image v0, page size 2048 | kernel + 9.3 MB recovery ramdisk + "second" |
| `resource` | `RSCE` | `rk-kernel.dtb` + boot logos |

The "second" stage of the boot images is about the size of the resource image; the vendor U-Boot takes the device tree from there. Kernel command line in the image header: `buildvariant=userdebug`; the rest comes from the DT `chosen/bootargs` and U-Boot.

## How the stock U-Boot starts an Android-image kernel

Stock U-Boot: Rockchip `next-dev` 2017.09, built 2024-03-06. Default environment: `kernel_addr_r=0x02080000`, `fdt_addr_r=0x01f00000`, `ramdisk_addr_r=0x0a200000`; `CONFIG_SYS_BOOTM_LEN` is 64 MiB for PX30. It does not check the boot image `id` hash.

An uncompressed kernel runs **in place** at `0x02080000`, and nothing aligns it first. That suits the vendor 4.4 kernel (`text_offset` 0x80000 on a 2 MiB base), but a mainline kernel has `text_offset` 0 and needs a 2 MiB-aligned start. The Image therefore sits at offset 0x180000 of the kernel blob (landing on `0x02200000`), and the blob is padded to the kernel's full `image_size`, so a ramdisk placed after it cannot overlap the running kernel. In front of it runs the chain loader (see [Bootloader logo and chain loader](#bootloader-logo-and-chain-loader)); the small bring-up test image (`tools/build_test_image.sh`) uses a single branch instruction instead (`tools/arm64_branch_pad.py`).

## The recovery slot and the default boot

The system lives in the **recovery** partition; `boot`, `kernel` and U-Boot stay untouched.

- **Install**: write the image to `recovery` from the running stock Android over Wi-Fi ADB (`tools/flash_recovery_adb.sh`: `adb push`, then `su -c 'cat img > /dev/block/by-name/recovery'` and an md5 check; the partition is not mounted while Android runs). No cable and no opened case needed, for anyone with ADB enabled through the eWeLink developer mode. The USB route (`rkdeveloptool wl` at sector 204800, BOOT button) is the rescue path (`docs/restore.md`). From a running PanelOS system: `tools/flash_recovery_ssh.sh`.
- **One-shot boot**: `adb reboot recovery` from Android, or `reboot-to recovery` from PanelOS. The DT uses `syscon-reboot-mode` (PMU GRF offset 0x200, recovery = `0x5242c303`).
- **Default boot**: the stock U-Boot reads an Android bootloader message (BCB) at **16 KiB into the `misc` partition** (Rockchip `boot_mode.c`, `BCB_MESSAGE_BLK_OFFSET`). While its command is `boot-recovery`, every start, cold or warm, boots the recovery partition, and nothing clears it (only the stock recovery would). `nspanel-bootmode open` writes that command, `nspanel-bootmode android` clears it, `status` shows it.
- The stock Android restores its own recovery at every boot (`flash_recovery` service: `/system/bin/install-recovery.sh` applying `/system/recovery-from-boot.p`). Without the boot command an image in `recovery` therefore only survives until the next Android boot: write it and boot it in one go.
- `boot` is only 32 MB. With the LZ4-packed kernel (see below) the image is about 30.5 MB and fits (`tools/build_image.sh` warns when it does not). The stock boot and recovery images have the same layout (Android boot image v0, page 2048, kernel 19 728 392 bytes, second stage 1 054 720 bytes in both), and the stock U-Boot has no `skip_initramfs`, so both partitions are loaded the same way. `tools/remove_android.sh` writes the running image there once the stock Android goes (install guide, step 14), which gives two slots (see `docs/decisions.md`). Tried on one panel: with the misc command cleared, a plain `reboot` (it writes the "normal" reboot mode, `mode-normal` in the DT) started PanelOS from the boot partition.
- The stock Android partitions (system, vendor, userdata, …) stay untouched unless the stock Android is removed; then userdata becomes `/mnt/data` (ext4, label `panelos-data`). A full backup (`tools/backup_partitions.sh`) is the way back to stock.
- **Test-boot safety net** (`S41testboot`): when the default boot is not the open system (a one-shot test boot) and there is no network within 90 s, the panel reboots into the stock Android, so the previous image can be restored over ADB without anyone at the panel. A hang in U-Boot itself is not covered: that needs a power cycle.

## Logs without a serial cable

The vendor kernel keeps a `ramoops` area at physical `0x110000`, size `0xf0000` (record 128 KiB, console 512 KiB, pmsg 320 KiB, no ECC). The mainline DT reserves the same region with the same layout. After a **warm** reset (power cycling clears RAM) the stock Android exposes the previous console log in `/sys/fs/pstore`, readable over ADB with `su` (`tools/read_pstore_adb.sh`).

A serial console (UART2, 1500000 baud; where its lines come out on the board is not known yet, see docs/hardware.md) remains the fallback when this is not enough.

## Power

A laptop USB port does not carry a full boot; use a 2 A charger.

## Display

- Both VOPs and the DSI bind, `card0-DSI-1` connects at 480×480; the ST7701S descriptor (`kernel/patches/0001`) works as is.
- Kernel text is not shown on the display (`console=tty0` not set, `vt.global_cursor_default=0`, `CONFIG_LOGO` off); kernel messages go to the serial console and pstore.

## Touch (CST226SE)

- The controller answers at I2C1 0x5a. The mainline `hynitron_cstxxx` driver fails: `cst3xx_bootloader_enter unable to enter bootloader mode`.
- The stock driver strings (`hyn_ts`, `cst2xx_ts_init`, firmware update command `0xD110`) place the CST226SE in Hynitron's CST2xx family, but it speaks the CST3xx commands and frames: chip ID via `D2 04` = 0xA8, 480×480, firmware 0x03000001. The CST3xx bootloader step (`A0 01 AA`) leaves it stuck in bootloader mode until a reset.
- `kernel/patches/0002` adds `hynitron,cst226se` without the bootloader step (reset delay 100 ms); the DT sets `touchscreen-swapped-x-y`. Five-finger multitouch, X 3..472, Y 5..480.
- The touch IRQ also fires about every 1.5 s without a touch (controller heartbeat), so the IRQ count alone does not prove touches.

## Restart through PSCI

With `system-power-controller` on the RK809 node, mainline `rk8xx-core` registers a PMIC restart handler (`DEV_RST`, priority 192) above PSCI (129). The PMIC reset clears the PMU GRF register at 0xff010200, so U-Boot sees a normal boot and `reboot-to recovery` lands in Android. The DT therefore has no `system-power-controller`; restart goes through PSCI and keeps the boot-mode flag.

## Wi-Fi (RTL8723DS, rtw88 over SDIO)

- Since Linux 7.x `pwrseq_simple` turns a single `reset-gpios` entry into a reset control served by the generic GPIO reset controller; it must be built in (`CONFIG_RESET_GPIO=y`). `cfg80211` needs the Buildroot `wireless-regdb` package.
- **Warm restarts**: WL_REG_ON (GPIO0_A2) is in the PMU domain and keeps its level across a PSCI restart, so the module stayed powered with its old firmware and the probe failed (`failed to dump efuse logical map`, -22). WL_REG_ON is now a fixed regulator (`vcc_wl`, `vmmc-supply` of the SDIO host) with 200 ms minimum off time and 100 ms start-up delay.
- **Self-heal** (`S40wifi`): if `wlan0` is not there after 8 s, the SDIO host (`ff380000.mmc`) is unbound and bound again, which switches `vcc_wl` off and on and probes the card again (up to 2 attempts; `wlan0` is back 2 s after the bind). `stop` waits for the old processes, so a restart never runs two `wpa_supplicant`s.
- **DHCP**: busybox `udhcpc` with default settings gave up after 3 tries and paused 20 s; with `-t 30 -T 1 -A 2` the address comes 1.7 s after association.
- `S40wifi` runs in the background, so the rest of the system does not wait for the network. Wi-Fi never reboots the panel; it keeps retrying.
- **Roaming**: wpa_supplicant changes the access point only when the link breaks (no `bgscan`), so after an access point outage the panel stayed on a farther one for hours. The app checks about every 30 minutes while it is idle (`app/src/roam.rs`, see docs/app.md); the move is a one-shot `wpa_cli roam`, and the network config never names an access point.
- **Log**: wpa_supplicant runs with `-s`, so connections, disconnections and access point changes are in `/var/log/messages` (Buildroot `BR2_PACKAGE_WPA_SUPPLICANT_DEBUG_SYSLOG`; `S40wifi` leaves `-s` out for a build without syslog support, since an unknown option would stop Wi-Fi). Without it, wpa_supplicant's messages went nowhere and the reason of an access point change could not be told afterwards.
- Settings are persistent (`S10persist` mounts the stock Android's `cache` partition, ext4, at `/mnt/persist`; files in `/mnt/persist/nspanel/`). The Wi-Fi network is imported once from the stock Android config (root-only, 600).

## Light and proximity sensor (STK3x3x)

Chip ID 0x5F is not in the mainline list; the `stk3310` driver warns "new unknown chip id" and works. Readings through IIO: room light about 545 raw (scale 0.1), covered 8, a flashlight saturates at 65535; proximity about 30 at rest, 140–235 with a finger about 5 cm away, 65535 when covered.

At the driver's default proximity setting (gain 1, 0.185 ms) the proximity reading stays 0 even with a hand on the glass. The app sets the most sensitive gain (`in_proximity_scale` 0.1), 11.84 ms integration time and the IR LED at full power.

**IR LED.** Register 0x03 (LEDCTRL; LED current in bits 7:6, pulse duty in bits 5:0 in Sensortek's STK331x register map) reads 0xAC on a running panel; the app writes 0xFF through i2c-dev. The driver never writes that register, so the value stays until power off. At 2.96 ms, a hand in front of the panel centre (mean of 20 readings):

| Distance | LED 0xFF | LED 0xAC |
|---|---|---|
| none (resting level) | 1818 | 1074 |
| 10 cm | +2150 | +1320 |
| 15 cm | +1135 | +680 |
| 20 cm | +282 | +171 |

Full LED power gives about 1.65 times the signal with the same noise.

**Integration time.** The resting level doubles from 2.96 ms (register 0x01 = 0x34, 1807 counts) to 5.92 ms (0x35, 3565) and 11.84 ms (0x36, 6990); 23.68 ms (0x37) reads the same as 11.84 ms. From 2.96 to 11.84 ms a hand's signal grows about 3.9 times, the noise (sd about 4 → 5 in a calm room) only about 1.3 times.

**Distance curve** at 11.84 ms, LED 0xFF: a hand in front of the panel centre, distance set with a ruler, signal above the resting level (three rounds, the one at 2.96 ms scaled by 3.87; single rounds differed by up to 30 % at 45–50 cm). These are the points of `PROXIMITY_CURVE` in `app/src/hw.rs`.

| cm | 10 | 15 | 20 | 25 | 30 | 35 | 40 | 45 | 50 | 55 | 60 |
|---|---|---|---|---|---|---|---|---|---|---|---|
| signal | 6629 | 2485 | 1406 | 771 | 433 | 271 | 183 | 114 | 66 | 38 | 26 |

**Noise.** It differed between measurements: about ±3 counts (2.96 ms) during the screensaver, about ±40 with a still message card on screen, ±200 in an earlier measurement; while a message card was redrawn every second, single readings with a hand at 15 cm were off by up to ±1000, also below the resting level. The cause is not established. Wake on approach therefore never uses a threshold below 5 times the measured jitter (see `docs/app.md`). With the screensaver on and the wake distance at 45 cm, the panel woke with a hand at about 45 and 40 cm.

**Over a day** (11.84 ms, LED 0xFF, one-minute means for 24 hours): the resting level was about 6885 in the dark and 6960–6980 in daylight from about 300 to 18 000 lx. Steady evening room light of 70–140 lx raised it by less than 20. The level follows the light within the same minute: when the light fell from about 520 to 140 lx within seconds, the resting level dropped by about 75 at once. The noise was sd about 5 in the dark and up to about 10 above 10 000 lx, so the wake margin stayed at the 45 cm signal (114) all day (5 times the jitter stayed below it). There was no approach report in 7.7 hours of darkness or in 8 hours of daylight, and every approach report that day came while someone was near the panel. From 23:00 to 6:00 the largest single reading was 18 above the minute's mean, in daylight 38. Direct sun on the glass and a sudden step from dim to bright daylight did not occur that day (the morning light rose gradually). In the ALS raw readings (one every 10 s) no single sample jumped to more than twice or less than half of its neighbours, except two samples of a few lux at about 5 lx.

**Touch test** (developer tool): while `/tmp/nspanel-touch-test` exists, the app shows nine points on black (the corners and edge middles 16 px from the edge, then the middle, held for 2 s), takes the touches itself and logs each one with its distance from its point (`touch test: point …`).

**Test view** (developer tool): while `/tmp/nspanel-proximity-test` exists, the app shows the reading, the resting level, the threshold and the estimated hand distance on a black screen and does not start the screensaver. First line of the file: the integration time in seconds (empty = the default; a change of the line, e.g. `0.011840 2`, learns the resting level anew); further lines: a prompt shown above the reading, so a script on the panel can guide the person through a measurement.

**Light level against other sensors.** The four ALS ranges switch cleanly (same light: 1408 counts at scale 0.1, 351 at 0.4, 86 at 1.6, 21 at 6.4; register 0x02 = 0x32/0x22/0x12/0x02), and the panel's own backlight does not reach the sensor (same reading with the backlight at 0, 10 and 100 %). The lux values, however, are higher than those of another light sensor held next to the panel, and not by a constant factor: from about 3.6 times to about 70 times, depending on the light (a narrow-beam spot lights two sensors side by side unevenly). In direct sun the panel read up to about 370 000 lx, while daylight cannot exceed about 120 000 lx. Sensors differ in how they take light from different directions and colours; which one is closer to the truth is not known without a calibrated meter. Automatic brightness and theme use the panel's values as they are.

## Audio (RK809)

- **Stock DT wiring**: RK809 codec (`mic-in-differential`, `adc-for-loopback`, `pdmdata-out-enable`), simple card with "Mic Jack" and "Headphone Jack". Playback on I2S1; capture through the SoC PDM controller.
- **Mainline**: `rk817` codec driver (it covers the RK809 MFD cell), I2S1 with SDI, `simple-audio-card`, MCLK from `SCLK_I2S1_OUT`. Card `rk809` with Master Playback/Capture Volume, Mic Capture Gain, Playback Mux (HP/SPK).
- The built-in speaker is on the codec's class-D output (**SPK**); HP is not connected. `S30audio` selects SPK at boot.
- The codec's capture input is **not a microphone**: it is the speaker echo reference (`adc-for-loopback`; a speech test left it at −63.5 dBFS, while tones on SPK came back nearly flat from 150 Hz to 8 kHz, about +27 dB from DAC to ADC).

## Microphones (PDM)

- The stock vendor partition has an AISpeech voice HAL with a "dual card" capture (`ff0a0000.pdm-rk817-hifi`) and an echo-canceller config for 4 capture channels, 2 microphones and 1 reference.
- `px30.dtsi` has no PDM node, so the board DTS adds it (`rockchip,px30-pdm`, values from the stock DT) with a generic `dmic-codec` and a second simple card `pdm-mics`. Data line 0 (GPIO2_C5) stays with the I2S1 SDI; both PDM clock outputs and data lines 1–3 are muxed.
- Only channels 2 and 3 (data line 1, GPIO3_D0) carry a signal: room noise about −64 dBFS, speech −56…−63 dBFS without gain. The two channels are highly correlated (0.94, no delay at 16 kHz); whether they are two capsules or one capsule read on both clock edges is not proven. The app uses their mean with a fixed gain.
- Speaker measured with the microphones: 1.5 kHz on SPK at Master Playback Volume 200 and −6 dBFS → −31.5 dBFS at the microphones; on HP nothing.

## Wake word benchmark

- Tool: `tools/wakeword_bench/` (C++, TensorFlow Lite 2.11 from Buildroot, TFLite micro frontend with the ESPHome `micro_wake_word` feature settings, one thread). Models: microWakeWord v2 `okay_nabu`, `hey_jarvis` and `vad`.
- CPU: governor `schedutil`, OPPs 600–1296 MHz.
- **Synthetic** (random input, clock at 1296 MHz): okay_nabu 161 µs, hey_jarvis 127 µs, vad 80 µs per inference; one inference per model every 30 ms → 1.23 % of one core for all three.
- **Live** (microphone, gain ×8, real room sound): frontend 164 µs per 10 ms step; okay_nabu 513 µs, hey_jarvis 374 µs, vad 248 µs per inference (the CPU clocks down between the short bursts). Whole process **5.6 % of one core**.
- No false wake on room speech (highest okay_nabu 0.62, cutoff 0.97); both words were detected when spoken, with no cross-detection.
- `arecord` writes a WAV header to stdout unless `-t raw` is given; the 44-byte header shifts 4-channel frames by two channels.

## Boot time

- Power on to the dashboard (cold boot, stopwatch): **PanelOS about 12 s, the stock firmware about 53 s** (Sonoff logo and animation about 36 s, then the Rubik boot animation), more than four times faster. PanelOS shows the dashboard with the right time and current Home Assistant data from the first frame.
- PanelOS: the logo appears after about 3 s (stock bootloader), about 1 s black, then the boot animation. A reboot command to kernel start takes 4.3 s, of which the bootloader is about 2.5 s.
- From kernel start: `/init` at about 1.6 s, the boot splash right after, Wi-Fi associates at about 6.7 s, the app has the clock (from Home Assistant) and the first states at about 7.4 s, the UI takes over at about 8.4 s.
- With the mainline device tree the stock bootloader phase took about 45 s and grew with the image size, about 1 s per MB (about 50 s with a 49 MB image). With the stock device tree (chain loader) it takes about 2.5 s with the same 41 MB image.
- The kernel is trimmed to the board (`kernel/configs/nspanel.config`): no USB, Ethernet, SCSI/SATA, MTD, SPI, NFS, SquashFS, netfilter, EFI, kexec, hibernation, MD, HID, remoteproc, SCMI, virtio, KALLSYMS_ALL, profiling, IKCONFIG, security modules; optimised for size. Image 17.8 MB instead of 28.9 MB. `CONFIG_MODULES=n` is a trap: it turns every defconfig `=m` into `=y` and makes the kernel larger.
- A first boot of a new kernel goes as a one-shot boot (`nspanel-bootmode android`, then `reboot-to recovery`): a failure ends in the stock Android after the panic reboot or the watchdog reset.

## Boot splash

- `buildroot/board/nspanel/src/nspanel-splash.c`, started by `S00splash` right after `/init` (backlight on at the same time): a static binary with plain DRM ioctls that plays `usr/share/nspanel/splash.bin`.
- `tools/splash/storyboard.py` renders the animation pixel-exact from `assets/logo.svg` and `assets/logotype.svg` (own SVG rasteriser `svgraster.py`, both fill rules, 8× supersampling) as delta frames; the player only copies changed pixels.
- The splash plays until the app asks for the display and Wi-Fi is up (a default route on `wlan0` in `/proc/net/route`).
- The app asks only when it has something current to show: it starts the Home Assistant client first and waits until the clock is set (the panel has no battery-backed clock and starts in 1970) and the first full state list is in, plus 300 ms for the assistant settings and forecasts; at most 25 s, at once when pairing is needed (the QR code is the content). While the network is not up yet, the client retries every 0.5 s.
- The clock: until NTP has set it, the app takes the time from the `Date` header of Home Assistant's WebSocket handshake (`app/src/clock.rs`). NTP starts about 1 s after Wi-Fi and needs a few more seconds; Home Assistant's answer comes earlier, and it also works on networks without internet access, where NTP never answers.
- Hand-over: the app sends SIGUSR1. Once Wi-Fi is up (checked at the end of each pass), or 30 s after the splash started, the splash gives up DRM master, keeps the picture and creates `/var/run/nspanel-splash.done`. The app waits up to 40 s for it, shows the same picture (`app/assets/intro.rgba`) as its first frame and fades it into the UI. The splash logs the hand-over to the kernel log (`nspanel-splash: display handed over …`).
- `splash.bin` v3 (1.36 MB) holds the animation's frames.
- The kernel's framebuffer console still binds (`Console: switching to colour frame buffer device 60x30`). With `CONFIG_LOGO` on, the penguin logo stayed in its buffer and showed again for a moment at shutdown, when the app released the display.

## Bootloader logo and chain loader

- The stock U-Boot drives its own drivers with the device tree of the boot image (`Using kernel dtb`). It draws a logo only when that device tree carries the vendor display description: `display-subsystem/route/route-dsi` with `logo,uboot = "logo.bmp"`, `logo,mode = "center"`, `connect` to the VOP-big DSI output, DSI panel `simple-panel-dsi` with `panel-init-sequence` and `display-timings`. From the stock resource image: `logo.bmp` 480×480, 8-bit palette, uncompressed.
- Mixing these vendor nodes into the mainline device tree (route-dsi, `rockchip,drm-logo` reserved memory, a vendor `simple-panel-dsi` node, vendor PHY names and compatibles) hung the stock U-Boot before it loaded the kernel. Without a serial console the node at fault cannot be found (the console is not reachable on this board, see `docs/hardware.md`).
- What works: U-Boot gets the **unchanged stock device tree**, the kernel its own. The boot image's second stage (resource image) carries the stock `rk-kernel.dtb` and `logo.bmp`; the kernel slot carries the chain loader (`tools/chainload/`), the mainline device tree and the Image (`tools/arm64_chain_pack.py`: loader at 0, device tree at 0x100000, work area at 0x140000, Image at 0x180000).
- The chain loader takes what U-Boot fills in at boot from U-Boot's device tree and puts it into the kernel's: the memory reservations (U-Boot's device tree, the initramfs), `/chosen` (command line, initramfs location), `/memory` (the DRAM banks without the firmware's regions) and the serial number. The result was checked against the device tree U-Boot hands over directly: the same content; the kernel's device tree then sits in the work area (`0x021c0000`). The loader runs with the MMU and data cache off (Linux arm64 boot protocol), hence `-mstrict-align`; on any failure it hands over U-Boot's device tree unchanged.
- The stock device tree comes from the user's own backup at build time (`tools/make_uboot_dtb.sh`, called by `tools/build_image.sh`); it is not in the repository. Two changes: `/chosen/bootargs` removed (stock Android arguments such as `console=ttyFIQ0` and `swiotlb=1`, which U-Boot would put in front of the PanelOS command line), and the backlight starts at 180 of 255 instead of 20, the brightness of the boot animation (the kernel's own device tree: 706 of its 1000 levels; `S20backlight` the same share).
- Side effect: with the stock device tree the bootloader phase dropped from about 45 s to about 2.5 s. The cause is not verified (no serial console); the earlier growth of about 1 s per MB of image points at slow eMMC reads with the mainline device tree.
- Watchdog: the loader starts the SoC watchdog (`dw-wdt` at `ff1e0000`, longest timeout: about 21 s at its ~100 MHz clock) only when the kernel's device tree enables it (`&wdt`). The kernel feeds it (`DW_WATCHDOG`, `WATCHDOG_HANDLE_BOOT_ENABLED`) until user space opens `/dev/watchdog`, which nothing does. A boot that hangs before the driver is up resets the panel. Tested by opening `/dev/watchdog` without feeding it: the panel reset after about 21 s and started PanelOS again. The PSCI restart handler (priority 129) stays ahead of the watchdog's (128), so the reboot-mode flag (`reboot-to recovery`) keeps working.
- Packed kernel (default, `KERNEL_PACK=lz4` in `tools/build_image.sh`): the kernel slot holds the loader, the device tree and the Image as one LZ4 frame (`lz4 -12 -BD`: 17.8 MB → 7.8 MB) behind a small header. The loader unpacks it to the start of the first DRAM bank (`0x200000`, 2 MiB aligned, below the loader) and jumps there. With the MMU and caches off (as the boot protocol hands over) unpacking took 7.4 s; the loader therefore turns on an identity map with caches for the unpacking (1 GiB blocks, DRAM write-back, the rest device memory, EL2), then turns the data cache off, cleans and invalidates what it touched (kernel, loader, packed data) and turns the MMU off again: 0.95 s. Reboot command to kernel start: 4.35–4.6 s, the same as with the uncompressed 41 MB image (U-Boot loads the 10 MB smaller image that much faster). The loader's own time is in `/proc/device-tree/chosen/panelos,loader-us`. The decoder is checked on a PC against the real kernel (bit-identical).
- Between the bootloader logo (`assets/bootlogo.bmp`) and the boot animation there is about 1 s of black: the mainline display driver resets the display at about 0.7 s, and `/init` starts at about 1.6 s, after the 20 MB initramfs is unpacked. A seamless hand-over would need kernel changes to the VOP, DSI and panel drivers.

## Build pitfalls

- `tools/build_image.sh` strips Windows line ends only from text files (`grep -I`). Stripping every file once removed the 0D byte of each 0D 0A pair in `splash.bin`, and the animation stopped after its first frame. Compare binaries in the target with the source (md5).
- The syslog-to-kmsg forwarder must skip `kern.*` lines and every line containing ` kernel:` or `syslog:`, otherwise it feeds its own output back (klogd → syslog → kmsg → klogd) and keeps a core busy.
- Restart services over SSH with `setsid`, otherwise the app supervisor dies with the SSH session.
