# Hardware

Measured on one Sonoff NSPanel Pro 86 unit (stock firmware 3.0.0, Android 8.1, vendor kernel 4.4.138) through ADB with root.

## Main components

| Part | Chip | Bus / address | Stock driver | Notes |
|---|---|---|---|---|
| SoC | Rockchip PX30, 4x Cortex-A35 @ 1.5 GHz | | | DT model string: "Rockchip PX30 evb ddr3 board" |
| GPU | Mali-G31 MP1 (Bifrost) | | mali | mainline: Panfrost |
| RAM | 2 GB DDR3 | | | |
| Storage | eMMC, 15269888 sectors (~7.3 GiB) | mmcblk1 | | |
| Display | 480x480 MIPI-DSI panel, Sitronix ST7701S | DSI | vendor | mainline `panel-sitronix-st7701` with a new descriptor (`kernel/patches/0001`) |
| Touch | Hynitron CST226SE | i2c-1 0x5a | cst226se | mainline `hynitron_cstxxx` with `kernel/patches/0002`; an unbound "CST226" node also sits at i2c-1 0x1a |
| Light + proximity | Sensortek STK3x3x | i2c-2 0x47 | sensors | stock exposes input devices "lightsensor-level" and "proximity"; the app sets the IR LED to full power (register 0x03 = 0xFF) and 11.84 ms proximity integration, see bringup.md |
| Light + proximity (alt part) | Sensortek STK3A5X | i2c-2 0x46 | none bound | second-source footprint, unused on this unit |
| Unknown | "tp" | i2c-2 0x3c | smatek_dridev | vendor device, function unknown |
| PMIC + audio codec | Rockchip RK809 | i2c-0 0x20 | rk808, rk809-codec | power key is input "rk8xx_pwrkey"; codec on I2S1 (MCLK GPIO2_C3, SCLK C2, LRCK C1, SDO C4, SDI C5); its ADC input is the speaker echo reference, not a microphone |
| Microphones | digital (PDM) | SoC PDM controller ff0a0000, data line 1 = GPIO3_D0 | AISpeech voice HAL (AEC: 2 mics + 1 reference) | verified by a speech test (channels 2+3); clock output (GPIO2_C6 or GPIO3_C7) not identified yet |
| Wi-Fi + Bluetooth | Realtek RTL8723DS | SDIO (024c:d723), BT on UART | 8723ds (out of tree) | mainline: rtw88 8723DS SDIO (works) |
| Zigbee | Silicon Labs EFR32MG21 (module SM-044 V1.1) | UART (one of ttyS3/4/5) | userspace | ships with EmberZNet 6.10.1 NCP firmware, see [Zigbee module](#zigbee-module); optional, router role at most |
| Ethernet MAC | rk_gmac-dwmac | | | present in DT, no connector expected |
| Backlight | PWM | /sys/class/backlight/backlight | pwm-backlight | 0..1000 (PWM clock 100 MHz: 2500 steps per period) |
| Thermal | soc-thermal, gpu-thermal | /sys/class/thermal | | |

## Board and opening the panel

Board marking on a pre-production unit: `ST-PX30-T6E-V1.0`, dated 2021-07. Details below come from blakadder's 2022 teardown photos (https://blakadder.com/nspanel-pro-sideload/) and the vendor device tree; check them against your own unit.

- The wall base holds the mains-powered part. The display unit gets low voltage through the 8-pin connector in the middle of its back. Disconnect mains before you work on anything.
- Opening the display unit: remove the two screws in the top corners of the back cover, pry the cover off starting at the notch on the top edge, then remove the five small screws that hold the board.
- To lift the board, peel the tape off the touch-panel connector (marked `TP`, bottom right), flip up its latch and pull the orange flex out. Do not touch the display flex (marked `LCD`, right edge).
- `OTG`: micro-USB port on the bottom edge of the board, used for ADB and Rockchip USB flashing.
- `BOOT`: tactile button on the right side, next to the `LCD` connector. Verified: holding it while plugging in the OTG USB cable (no other power) makes the panel enumerate as `2207:330d` "USB download gadget": the vendor U-Boot's rockusb Loader mode (`rkdeveloptool ld`), not the boot ROM's mask ROM mode. Loader mode depends on an intact U-Boot on the eMMC.
- Power over the OTG port: a laptop USB port is enough for U-Boot download mode, but **not for a full boot** (the stock Android hangs on the Sonoff logo indefinitely). From a 2 A USB charger it boots normally. For work that needs data and full power at the same time, use a powered USB hub.
- The labelled pads at the top left (`GND`, `RX`, `TX`, `VCC`, `RST`) belong to the Zigbee module next to them (module "SM-044 V1.1", Silicon Labs EFR32MG21 chip): they are its serial and reset pads (see [Zigbee module](#zigbee-module)). `GND` is the pad directly below its label and is connected to a GND pin of the 8-pin base connector. They are **not** the SoC's console: with a USB-UART adapter on them nothing arrives while the kernel writes to its console.
- SoC console: UART2 (`ff160000`), **1500000 baud, 8N1**, 3.3 V logic (vendor bootargs `console=ttyFIQ0 earlyprintk=uart8250,mmio32,0xff160000`). Its TX comes out on two pins: GPIO1_D2 (UART2_M0, used by the kernel DT and by the vendor DT's fiq-debugger) and GPIO2_B4 (UART2_M1; the kernel never touches it, so the stock boot chain sets this mux). Both were confirmed by writing zeros to `/dev/ttyS2` and reading the pin levels. Neither line reaches any test pad or pin on either side of the board (`IO1`/`IO2` included), so the console is not usable without a board layout. The method used, for other board revisions: write zeros to `/dev/ttyS2` for 2 s, pause 2 s, repeat; a TX point then alternates between 3.3 V and about 0.3 V on a multimeter. A USB-UART adapter must support 1.5 Mbaud (FT232, CH340, CP2102N).
- On reassembly, seat the light/proximity sensor board (marked `ALS`, top centre) correctly.

## Zigbee module

Read through the module's pads with a 3.3 V USB-UART adapter while the panel ran from its charger. Only queries were sent; nothing was written to the module.

- Pads: `TX` is the module's output, `RX` its input (adapter TXD to `RX`, adapter RXD to `TX`, plus `GND`). `VCC` stays unconnected while the panel is powered.
- Serial line: 115200 baud, 8N1, ASH framing. The module sends XON (`0x11`), so it uses software flow control; no RTS/CTS lines are needed.
- Firmware: EmberZNet 6.10.1.0 (build 216, GA release) as an NCP, EZSP protocol version 8.
- Bootloader: Gecko Bootloader v1.12.00 with the serial XMODEM menu (`1. upload gbl`, `2. run`, `3. ebl info`) at 115200 baud. The running firmware enters it with the EZSP command `launchStandaloneBootloader`; menu item `2` starts the firmware again. The menu offers a GBL upload, so replacing the firmware should not need SWD (not tried yet).
- EZSP `getStandaloneBootloaderVersionPlatMicroPhy` returns bootloader version `0x1C00`, platform 4, micro 24, phy 15.
- Manufacturing tokens `MFG_STRING`, `MFG_BOARD_NAME`, `MFG_MANUF_ID` and `MFG_CUSTOM_VERSION` are empty (all `0xFF`).
- Which SoC UART the module is wired to is not known yet. The vendor DT enables UART1, UART4 and UART5; UART1 (GPIO1_C0/C1) belongs to Bluetooth. Holding the module's `RX` line low from the adapter changed none of the SoC's GPIO input levels, the UART4 (GPIO1_D4/D5) and UART5 (GPIO3_A1/A2) pins included. The PanelOS device tree enables only the console UART, so the SoC leaves the module's lines alone.

## Display panel

Decoded from the vendor device tree (`resource` partition, `rk-kernel.dtb`, node `dsi@ff450000/panel@0`, `simple-panel-dsi`).

- Controller: **Sitronix ST7701S** (the init sequence uses its `FF 77 01 00 00 1x` bank-select commands). Mainline has a driver for this family, `drivers/gpu/drm/panel/panel-sitronix-st7701.c`; `kernel/patches/0001` adds this panel's descriptor.
- 480x480, 72 x 70 mm, MIPI-DSI 2 lanes, RGB888, flags `0x803` (video mode, burst, low-power commands).
- Timing: pixel clock 16 MHz; horizontal front porch 12, sync 10, back porch 15; vertical front porch 12, sync 8, back porch 15; all sync signals active low.
- Power: RK809 `SWITCH_REG2`. Reset: GPIO3_A0, active low. Delays: reset 20 ms, init 20 ms, prepare 20 ms, enable 40 ms.
- Backlight: `pwm-backlight` on PWM1 (`pwm@ff200010`), period 25 us (40 kHz), PWM clock 100 MHz (2500 duty steps). PanelOS uses 1000 levels; 0.8 % (8 of 1000) is still readable at night and shows no flicker.

Init sequence (DCS, in order; delays in ms after the command):

| Cmd | Data | Delay |
|---|---|---|
| FF | 77 01 00 00 10 | |
| C0 | 3B 00 | |
| C1 | 0D 02 | |
| C2 | 21 08 | |
| B0 | 00 11 18 0E 11 06 07 08 07 22 04 12 0F AA 31 18 | |
| B1 | 00 11 19 0E 12 07 08 08 08 22 04 11 11 A9 32 18 | |
| FF | 77 01 00 00 11 | |
| B0 | 60 | |
| B1 | 30 | |
| B2 | 87 | |
| B3 | 80 | |
| B5 | 49 | |
| B7 | 85 | |
| B8 | 21 | |
| C1 | 78 | |
| C2 | 78 | 20 |
| E0 | 00 1B 02 | |
| E1 | 08 A0 00 00 07 A0 00 00 00 44 44 | |
| E2 | 11 11 44 44 ED A0 00 00 EC A0 00 00 | |
| E3 | 00 00 11 11 | |
| E4 | 44 44 | |
| E5 | 0A E9 D8 A0 0C EB D8 A0 0E ED D8 A0 10 EF D8 A0 | |
| E6 | 00 00 11 11 | |
| E7 | 44 44 | |
| E8 | 09 E8 D8 A0 0B EA D8 A0 0D EC D8 A0 0F EE D8 A0 | |
| EB | 02 00 E4 E4 88 00 40 | |
| EC | 3C 00 | |
| ED | AB 89 76 54 02 FF FF FF FF FF FF 20 45 67 98 BA | |
| FF | 77 01 00 00 00 | |
| 11 | (sleep out) | 120 |
| 29 | (display on) | 120 |

## Touch controller: interrupt bursts (open)

The kernel log sometimes fills with `Hynitron-TS 1-005a: cst3xx touch read failure`, hundreds of lines a second for up to several seconds. Four bursts, measured on one panel (kernel log, `/proc/interrupts`):

| Burst | Lines | Length | Next to it |
|---|---|---|---|
| 1 | ≈590 | ≈4 s | app start |
| 2 | ≈720 | ≈4 s | proximity "approach" in the same second |
| 3 | 240 | 0.35 s | proximity "approach" 1 s later |
| 4 | 3012 | 7.2 s | proximity "approach" at 65535 counts (the sensor's maximum: a hand on the glass), again 2 s later |

The touch interrupt had fired 68 716 times in 16.3 h of uptime. Three of the four bursts came with the proximity sensor seeing a hand, so while the panel was touched or covered; the first came while the app started.

What the driver does (`drivers/input/touchscreen/hynitron_cstxxx.c`, `cst3xx_touch_report`): on every interrupt it reads 28 bytes from register 0xd000. It prints this one message both when the I²C read fails and when the frame does not check out: byte 6 is not 0xab, or byte 0 is 0xab (a frame that was already read). In both cases it returns **without** writing the end-of-read command (0xab to 0xd000), so the controller keeps its interrupt line low (the interrupt is level triggered) and the read runs again at once. That explains the rate. The frames of a burst are dropped, so touches during a burst are lost. A frame read while nobody touches the panel starts with 0xab (`ab 01 1c 70 3b 00 ab 00 00 00 2e 35 ab …`).

Frames read during a burst (read-only watcher: `i2ctransfer -f -y 1 w2@0x5a 0xd0 0x00 r28`, triggered by the interrupt rate): `ab 0e 0f 40 40 01 ab 00 …` and `ab 1d 0f 48 19 00 ab 00 …`. Byte 0 still holds the end-of-read mark 0xab while byte 5 reports one touch (then none): the controller raises its interrupt for frames whose status byte it did not refresh, so they are not I²C errors. Such frames come while a finger rests on the glass, which fits a long press that does not register.

A test with known touches (the four corners, then a long press in the middle; the driver logged the rejected frames): `ab 00 00 be 2a 01 ab 00` decodes to one touch at (11, 14), the top left corner, and `ab 10 0e e3 47 01 ab 00` to one touch at (270, 227), the long press in the middle; each came again every 1.2 ms (1142 more lines within 5 s). So these frames are real touches with a stale status byte, and dropping them is why corners seemed insensitive and a long press did not register. Ending the read alone did not let the line go: the controller keeps it low while the finger rests.

Fix (`kernel/patches/0002`, `end_bad_frames` for the CST226SE): a frame whose status byte holds the end-of-read mark and that reports one touch is reported as that touch (one finger, slot 0, coordinates and width from bytes 1–4); one that reports none releases all contacts; the read is ended and the next report waits about 10 ms, so a resting finger gives about 100 reports a second instead of about 800. Other rejected frames are logged with their first bytes, rate limited.

A second test (evtest on the panel) with this fix: every corner came through at the edge, (21, 8), (450, 8), (11, 471), (439, 471), and the long press lasted 5.4 s with 126 reports. But 1 ms after every release the controller sent the last frame once more, which became a 13 ms ghost touch at the same place, a second tap. The driver therefore ignores a frame without status that repeats the last contact's place within 60 ms after it went.

Accuracy, with that fix (the app's touch test view, `docs/bringup.md`): nine points 16 px from the edges and the middle, four rounds, touched in different ways (one round with the other hand, speeds varied, as in everyday use). Every point registered at the first touch, 36 of 36, and the event log shows no ghost touch (27 touches for the last 27 points; the shortest pause between two touches 391 ms). Offsets of the touch from the point:

| | x | y |
|---|---|---|
| top row (12 touches) | +0.2 ± 4.1 px | −7.4 ± 2.5 px |
| middle row (12) | −0.9 ± 10.6 px | +11.2 ± 13.7 px |
| bottom row (12) | +2.1 ± 5.5 px | +7.2 ± 0.7 px |
| all 36 | +0.5 ± 7.2 px | |

The only systematic part is in y at the top and bottom edges: a touch there is reported 7–8 px nearer the edge, the same in every round and with either hand (y about 3 % stretched around the middle; x not). Everything else is where the finger landed. All of it is far below a fingertip (40–50 px), so on this panel a calibration would not help in everyday use. It is one panel, though: whether every unit behaves the same is not known (glass, sensor and controller tuning can differ from unit to unit). Hence a touch calibration in the settings, started on the panel or from the integration: it runs these nine points once and shifts all touches only when they are off the same way (more than 3 px on average, spread under 6 px).

## GPIO and pin map (from the vendor device tree)

| Function | Pin | Notes |
|---|---|---|
| Panel reset | GPIO3_A0 | active low |
| Wi-Fi power sequence reset | GPIO0_A2 | active low, `mmc-pwrseq-simple` |
| Wi-Fi host wake IRQ | GPIO0_A5 | |
| Bluetooth reset | GPIO0_C1 | |
| Bluetooth device wake | GPIO0_A1 | |
| Bluetooth host wake IRQ | GPIO0_B2 | |
| Bluetooth UART RTS (GPIO mode) | GPIO1_C3 | 32 kHz clock from the RK809 |
| MBI5041 (bit-banged SPI1) | GPIO3_B4 MOSI, GPIO3_B7 CLK, GPIO3_B5 CS | 16-channel LED driver, purpose unknown |

UARTs enabled in the vendor DT: `serial@ff158000`, `serial@ff170000`, `serial@ff178000` (Bluetooth and the Zigbee radio are among them). The vendor DT also lists alternative parts that are disabled on this unit: Goodix GT1x and GSL2682 touch, APDS9933 light sensor, SHT21 and CHT8305 temperature/humidity sensors, MMA7660 accelerometer, AK8963 compass, OV5695 camera.

## eMMC partition map

512-byte sectors. Measured on firmware 3.0.0; check your own unit before writing.

| Name | Start sector | Size (sectors) | Size |
|---|---|---|---|
| uboot | 16384 | 8192 | 4 MiB |
| trust | 24576 | 8192 | 4 MiB |
| misc | 32768 | 8192 | 4 MiB |
| resource | 40960 | 32768 | 16 MiB |
| kernel | 73728 | 65536 | 32 MiB |
| boot | 139264 | 65536 | 32 MiB |
| recovery | 204800 | 131072 | 64 MiB |
| backup | 335872 | 229376 | 112 MiB |
| security | 565248 | 8192 | 4 MiB |
| cache | 573440 | 786432 | 384 MiB |
| system | 1359872 | 3145728 | 1.5 GiB |
| metadata | 4505600 | 32768 | 16 MiB |
| vendor | 4538368 | 786432 | 384 MiB |
| oem | 5324800 | 524288 | 256 MiB |
| frp | 5849088 | 1024 | 512 KiB |
| smatek | 5850112 | 32768 | 16 MiB |
| keypart | 5882880 | 512 | 256 KiB |
| STSN | 5883392 | 512 | 256 KiB |
| stback | 5883904 | 2097152 | 1 GiB |
| userdata | 7981056 | 7288799 | ~3.5 GiB |

The first 16384 sectors hold the partition table and the Rockchip idbloader.

## Stock boot and update chain

- Non-A/B Android 8.1 layout. The recovery partition installs OTA packages and verifies them against Sonoff's RSA key, so only vendor-signed packages install through recovery.
- The stock launcher refuses OTA checks while its server-side "rooted" flag is set, and the same flag switches ADB on and off (`settings put global adb_enabled 0/1`).
- Network ADB is enabled by the init rule `on property:persist.internet.adb.enable=1` (sets `service.adb.tcp.port 5555`).
