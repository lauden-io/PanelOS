# Design decisions

| Decision | Reason |
|---|---|
| Replace the vendor firmware completely | Removes the software bloat; on the NSPanel Pro, Android 8.1 caps the WebView at Chromium 138 on every Sonoff firmware |
| Mainline Linux kernel from the start | Long-term maintainability, open GPU driver (Panfrost), no vendor kernel lock-in |
| Buildroot root filesystem | Small, fast boot, reproducible |
| Native Rust app with the Slint UI toolkit | No browser and no WebView; renders directly on DRM/KMS; small and fast on a Cortex-A35 |
| Home Assistant WebSocket API for the dashboard, ESPHome native API for the device and the voice satellite | Works with Home Assistant's built-in integrations; no broker and no custom integration needed for the basics |
| Zigbee radio optional, router role at most | Not needed for the panel's main job |
| Keep the stock U-Boot (no mainline U-Boot for now) | Replacing it is the only step that can brick the panel, and the USB rescue mode depends on it; it keeps the install simple for everyone. With the chain loader below it shows the PanelOS logo and takes about 2.5 s |
| The stock U-Boot gets the stock device tree, the kernel its own (chain loader in front of the kernel) | The stock U-Boot drives its own display and storage with the boot image's device tree: with the mainline one it showed no logo and took about 45 s, vendor nodes mixed into it made it hang. The stock device tree comes from each user's own backup, it is not redistributed |
| Kernel packed with LZ4 in the boot image, unpacked by the chain loader with caches on | The image fits the 32 MiB `boot` partition (needed for A/B) at the same boot time |
| The panel keeps trying for good when Home Assistant or the network is away | After a power cut Home Assistant or the router may take minutes or hours; the panel must come back by itself, without a restart |
| Hold the UI back until the clock and the first Home Assistant data are in | The first screen shows the right time and current states, not 1970 and empty tiles; the boot animation shows the wait |
| Features inspired by an open-source Flutter panel app, no code reused | Feature ideas only |
| Project licence AGPL-3.0-or-later | Slint is used under GPLv3 (its royalty-free licence excludes embedded systems); GPLv3 §13 permits combining it with AGPL-3.0 code |

## Kernel line

The current base is the latest stable kernel (7.3.x). There is no 7.x longterm kernel yet; by long-standing practice the last mainline release of a year becomes the next longterm kernel, so the 2026 one is expected to be 7.4 (a convention, not a promise). Plan: track the latest stable and move to the 2026 longterm kernel once it is announced (https://www.kernel.org/releases.json).

## Updates (planned)

- One signed bundle per release (kernel, DTB, root filesystem, panel app), published as a GitHub release. The panel verifies the signature before installing.
- A/B, updated in the background, with test boot and automatic fallback. PanelOS runs from RAM, so the panel stays fully usable while an update is downloaded, written and checked; only the restart interrupts it. The two slots are the recovery partition and, once the stock Android is removed (`tools/remove_android.sh`), the boot partition. The stock U-Boot has no slot retry counter, and its misc boot command wins over the one-shot boot, so only the recovery slot can be test-booted once. The order is therefore: the running version is copied to the boot partition (it becomes the backup), the new version is written to the recovery partition and verified, and the panel restarts into it once (one-shot recovery boot with the default boot cleared). It becomes the default only after a health check (Wi-Fi, app running, Home Assistant connected); if it fails or hangs, the panel restarts by itself and comes up from the boot partition with the previous version. Both restarts are automatic: into the new version at a quiet moment (or at once when asked), and back to the previous one when the new one fails, without anyone at the panel. A failed update is reported: before the test start the panel notes the version it tries on its persistent partition; a start with another version and that note still there means the update failed, and the panel says so on its screen and in Home Assistant (the update entity shows the failure, plus a notification with the reason), then clears the note. The version goes into each image's header (`name` field), so the panel can tell which slot holds what. Android's A/B ("seamless") updates work the same way, with the slot switching done by a bootloader that counts tries; here the one-shot boot and the health check take that part.
- Home Assistant: an `update` entity (installed version, latest version, release notes, install command, progress), so the update shows up under Settings → Updates like any other; auto-update is optional.
- On the panel: a settings page with the version, "check for updates", "install" and an auto-update switch.
- First install over Wi-Fi ADB from the stock firmware (no opening the case), every later update over the air.
- The panel can update itself without Home Assistant too: it checks GitHub Releases directly (or a local file/URL).

## User interface

- Native: Rust + Slint, drawn by the panel itself, far faster than the web view; dashboard mode stays the secondary way.
- The many pages bring many large images, photos and fonts. They live on disk in a bundle of their own (like the web bundle: SquashFS, on the data partition once the stock Android is removed), not in the boot image, and a page loads what it needs.
- Everything is optimised as far as the hardware allows: the heaviest page is built first and measured on the panel (frame times, CPU per core, memory, temperature, the voice satellite alongside), and the results set per-layer budgets and fallbacks for every other page.

## Home Assistant connection

- **Dashboard**: Home Assistant's WebSocket API.
- **Device and voice**: the panel speaks the ESPHome native API, so Home Assistant's built-in ESPHome integration adds it like any ESPHome device: settings and sensors as entities, plus the Assist satellite over the same connection. Several panels are handled like any ESPHome device; automations can switch modes.
- **No token typing on the panel.** Pairing uses Home Assistant's standard OAuth (IndieAuth) login: the panel shows a QR code, the user scans it with a phone and logs in to Home Assistant as usual, Home Assistant redirects to the panel (client_id and redirect_uri on the panel's own address), and the panel exchanges the code for a refresh token. The token is visible and revocable in the Home Assistant user profile.
- The QR login shows Home Assistant's normal login form, so any user can log in, for example a dedicated non-admin "panel" user, independent of the account the phone's Home Assistant app uses (a private browser window avoids any confusion). An on-panel login with a touch keyboard is planned as an alternative.
- **Dashboard choice:** the panel lists the Home Assistant dashboards; the user picks one on the panel's settings screen or with the Dashboard select in Home Assistant.
- **Kiosk by design:** the app draws the dashboard natively, not the Home Assistant web frontend, so there is no Home Assistant header or sidebar at all. Trade-off: only the card types the app implements render; custom frontend cards (HACS) do not.

## Display modes

| Mode | What it is | Engine |
|---|---|---|
| Native dashboard (default) | Predefined card types; content from a Home Assistant dashboard chosen for the panel; own look, themes, more-info dialogs | Rust/Slint app |
| Assistant | The panel as a smart display: big clock and date, the weather now and for the next hours, what is playing (artwork and controls), a few quick tiles, timers; while you talk to it, what it heard and what it answers, full screen | Same app; set in the PanelOS integration (`custom_components/panelos`) |
| Web (planned) | The real Home Assistant frontend as a kiosk, custom (HACS) cards included | WPE WebKit + cog on DRM, GPU through Panfrost; its speed on the PX30 has to be measured first |

- The **voice satellite** works in every mode: PDM microphones → on-device wake word → Home Assistant Assist pipeline, echo cancellation with the player's own output as reference, TTS and media through the speaker.
- **Configuration in three layers:**
  1. On the panel (settings screen): what must work without Home Assistant – Wi-Fi, pairing, mode, brightness, theme, screensaver, about.
  2. In Home Assistant as ESPHome device entities: mode, dashboard, theme, brightness, screensaver, wake words, volume, microphone mute, sensors.
  3. Content: Home Assistant dashboards; the assistant mode through the own integration.
- The look is a placeholder until a full redesign.

## Future plans

See the [README](../README.md#future-plans).

## Stock firmware redistribution

The stock firmware backup is not published: it is Sonoff's copyrighted software, and several partitions carry device-unique identity (MAC addresses, keys, serial numbers, licence flags) that must never be copied to another unit. Instead the project ships the backup tool so every user backs up their own panel, and the installer has to support the firmware versions people actually run. Downgrading 4.x to 3.0.0 is not a planned path.

## Install compatibility

- Required: Wi-Fi ADB **with root** (`su`), as on the stock 3.0.0 userdebug build. Some community reports say newer stock firmware removed basic tools (`dd`, `chmod`, ...); the installer should bring its own static tools.
- Only the 86 model (480x480, ST7701S) so far. The 120 model and the second generation are different hardware.
- Known to work: a unit rooted on an older stock firmware and then updated to 3.0.0 (the stock launcher blocks OTA updates on rooted units, so it stays there).
- Verified on stock firmware 3.0.0 only. The stock U-Boot on 4.x firmware may load kernels differently; the installer must check prerequisites and refuse to write anything if they are not met.

## Open questions

- Whether the two microphone channels are two capsules or one capsule read on both clock edges.
- Identify the device at i2c-2 0x3c and the Zigbee UART.
