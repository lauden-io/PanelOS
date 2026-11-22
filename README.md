<picture>
  <source media="(prefers-color-scheme: dark)" srcset="branding/kit/social/readme-header.png">
  <img alt="PanelOS: open firmware for smart home wall panels" src="branding/kit/social/readme-header-light.png" width="100%">
</picture>

Open-source Linux firmware for smart home wall panels: it replaces the closed vendor firmware with a minimal **mainline Linux** system built with **Buildroot**, running its own native interface for Home Assistant (**Rust + Slint**, drawn on the GPU) and a voice satellite. No browser, no cloud.

Version **0.0.7** (what is new: [CHANGELOG.md](CHANGELOG.md)). Licence: AGPL-3.0-or-later ([LICENSE](LICENSE)).

## Supported devices

| Device | SoC | State |
|---|---|---|
| Sonoff NSPanel Pro (86 model) | Rockchip PX30 | working: native interface, voice satellite, boot logo and animation, about 12 s boot |

## The interface

PanelOS draws its own interface on the panel's GPU: no browser, no web page, every pixel made for a 480 × 480 wall screen. The plain looks run at the screen's full 60 frames a second, the richest living scenes (drawn cities at night, the synthwave drive, neon timers) at 30 to 55, and a still screen does not draw at all ([measured view by view](docs/app.md#frame-rates)). Swipe through your pages (there is no last one: after it comes the first again), pull down the quick shade, talk to Assist. Nothing waits for a page to load.

<img alt="PanelOS on the NSPanel Pro: a synthwave home page, a turntable, rain on the glass, a wall thermostat with running timers, a mixing desk, the oscilloscope voice view, a pager and a split-flap clock" src="docs/images/panelos-views.png" width="100%">

**A custom page that looks outside.** Your clock, the weather and your widgets sit on a living picture of the sky as it is right now: rain on the glass, fog, clouds that travel with the wind, lightning, and the sun and the moon where they really are. Pick a photo of nature, the countryside or a city, a drawn scene, a quiet line drawing, a synthwave night drive with neon letters and cars on the road, or simply a plain ground in the panel's own colour. Want more than one? Add another custom page with its own background and its own widgets.

**Pages for everything you use, each in looks with character.**
- **Music**: a clean cover, the cover full screen, a turntable whose arm follows the track, a cassette and half a cassette; pick any player of the house, move the music to another one or play in several rooms at once.
- **Timers, alarms and a stopwatch**: a plain ring, a kitchen timer, an hourglass, a flip clock or a synthwave sunset; the stopwatch as a chronograph, a timing tower, an LCD sports watch or nixie tubes, the alarms as a twin-bell clock, a sunrise, a clock radio or a 24-hour dial; quick starts, your own lengths, alarms for chosen days, and timers set by voice from any room.
- **Thermostat**: a dial, a remote control, a wall thermostat with slide switches, or a neon one; room by room.
- **Cameras**: one big or all of them, and when the doorbell rings the panel wakes with the visitor on screen, and with a lock, hold to open.
- **Calendar**: agenda, day, three days, week and month, or a departure board, a tear-off calendar and a wall calendar with your events written in pen.
- **To-do lists**: a plain list, a shop receipt, a terminal, embossed label tapes, or a yellow notepad in handwriting.
- **Notifications**: a list, sticky notes on a fridge, a 90s pager, or the evening news.
- **Clock**: digital or analog faces, a split-flap board, the time in words, nixie tubes, an LCD quartz watch or a twin-bell alarm clock; on its own page, as a widget and on the screensaver.
- **Quick controls**: tiles, a mixing desk with faders, or a flight deck with toggle switches.

**It tells you what matters.** Status screens show what happens at home (someone in the bathroom, a call going on, the robot vacuum at work) in their own colour, with a few words; a swipe folds one into a strip on top. A critical alert (smoke, CO, gas, water) takes the whole screen in red and keeps sounding until someone holds the button. Do not disturb lets through what you choose.

**Talk to it.** Wake words run on the panel itself. While you speak the screen listens with you, in a plain, terminal, comic or oscilloscope look, and the answer appears as it is spoken.

**It is yours.** Every page, its order, its look and what it shows are set on the PanelOS page in Home Assistant's sidebar, with a live picture of the panel's screen beside the settings. And this is only a glimpse: there is a lot more in it than fits here.

**Weather: [Bellweather](https://github.com/lauden-io/Bellweather)** was made for exactly this. It tells the weather here and now from real observations (radar, nearby stations) and blends its forecast from many models, each weighted by how well it did lately. The living backgrounds read it, so they show what is really happening outside: use it for the best experience. Any other Home Assistant weather entity works too, only less precisely; how much less, Bellweather's repository shows with its measurements and the evidence behind them.

## More features

- **Voice satellite**: on-device wake words (microWakeWord), Assist pipelines, echo cancellation, timers, announcements, follow-up listening.
- **Dashboard mode** (secondary): a Home Assistant dashboard in a web view, custom cards included; within the limits of the panel's hardware, so a light, simple dashboard made for it works best ([docs/home_assistant.md](docs/home_assistant.md#dashboard-mode)).
- **First-start setup on the panel**: Home Assistant found on the network (mDNS) or its address typed on a keypad; pairing with a QR code.
- **Home Assistant device and page**: the PanelOS integration (`custom_components/panelos`) adds the panel as a device with its display, settings and sensors, and a PanelOS page in the sidebar where every panel's pages, looks, alerts, status screens, display and voice are set beside a live picture of its screen; it also shows views or messages on it; the voice satellite is an ESPHome device (announced over mDNS like any ESPHome device), connected via the panel.
- **Super fast boot**: about 12 s from power on to the dashboard, more than four times faster than the stock firmware (about 53 s). The PanelOS logo already shows in the bootloader, then the boot animation; the UI comes with the right time and current Home Assistant data. After a power cut it waits for Home Assistant and the network as long as it takes and comes back by itself.

Details: [docs/app.md](docs/app.md).

## Future plans

- More devices: other NSPanel models (NSPanel Pro 120, NSPanel Pro Gen2), the Shelly Wall Display (MediaTek MT6580; mainline Linux support still to be checked) and Wall Display XL (Rockchip RK3566), and the ESP32-based NSPanel as a separate, smaller target (its screen is driven by its own controller over a serial link).
- A 3D-printable adapter plate for mounting the US NSPanel on a European 60 mm wall box (screw spacing 83.5 mm vs 60 mm).
- Ready-made images and bundles on GitHub Releases, so a user does not have to build. A release image must not carry a personal SSH key: the user's own key is added at install.
- Native builds on Macs with Apple silicon and other arm64 computers: Buildroot's Bootlin toolchain only runs on x86-64 hosts, so an arm64 host needs Buildroot's own toolchain (built from source). Until then the build runs in an emulated x86-64 virtual machine (see [docs/install.md](docs/install.md#mac)).
- Signed over-the-air updates from Home Assistant's Settings → Updates, written in the background into the second system slot (A/B: the recovery partition and, once the stock Android is removed, the boot partition). The panel stays usable while it updates and is out only for the restart; a failed update restarts into the previous version by itself and says so.
- A wiki once more devices are supported.

## Why (NSPanel Pro)

- The stock software is slow: a heavy React Native launcher, constant cloud sync and an old Chromium 107 WebView.
- The panel runs Android 8.1 on every Sonoff firmware version (3.x and 4.x alike), so the system WebView can never go past Chromium 138.
- Enabling ADB/root permanently blocks official firmware updates.
- The hardware itself is fine for a wall panel: 4x Cortex-A35, Mali-G31, 2 GB RAM, 480x480 MIPI-DSI screen, capacitive touch, light and proximity sensors, microphones and a speaker.

## Install (NSPanel Pro 86)

Step-by-step guide, no Linux knowledge needed: **[docs/install.md](docs/install.md)**.

In short: turn on developer mode and ADB in the eWeLink app, back up the panel, build the image (it takes the stock bootloader's device tree from the backup) and the two bundles (the interface and the dashboard browser) on a Linux or WSL computer, write PanelOS to its recovery partition over Wi-Fi ADB, make it the default boot, put the bundles on it over SSH, type the address of Home Assistant on the panel's setup screen (or pick it when found), and log in with the QR code on the screen.

### The Home Assistant integration

**With HACS** (updates arrive like any other):

[![Open your Home Assistant instance and open this repository in HACS.](https://my.home-assistant.io/badges/hacs_repository.svg)](https://my.home-assistant.io/redirect/hacs_repository/?owner=lauden-io&repository=PanelOS&category=integration)

1. Click the button, or in HACS open the menu (⋮, top right) →
   **Custom repositories**, paste `https://github.com/lauden-io/PanelOS`,
   choose the type **Integration** and add it.
2. Find **PanelOS** in HACS and download it.
3. Restart Home Assistant.

**With the Marketplace** (Home Assistant 2026.11 and later): **Settings → Marketplace → Add from link**,
paste `https://github.com/lauden-io/PanelOS`, download **PanelOS**, then restart Home Assistant.

**By hand:** copy `custom_components/panelos` into Home Assistant's
`custom_components` folder, then restart Home Assistant.

**First setup:** Settings → Devices & services → Add integration → **PanelOS**. Panel ID: the panel's network name (`nspanel-pro` by default). The panel's pages, looks and settings are then set on the PanelOS page. Details: [docs/home_assistant.md](docs/home_assistant.md).

## Known limitations

- No TLS yet: Home Assistant must be reachable over plain `http://`.
- mDNS (finding Home Assistant on the setup screen, Home Assistant discovering the panel) only works inside one network segment, unless the router repeats mDNS between them.
- Web mode (the real Home Assistant frontend) is not started.
- Alarm sounds are simple beeps for now.

## Repository layout

- `app/` the panel app (Rust + Slint), see [docs/app.md](docs/app.md)
- `custom_components/panelos/` Home Assistant integration (the panel's device, its PanelOS page, shared alerts and status screens, screen actions), installable with the Home Assistant Marketplace or HACS, see [docs/home_assistant.md](docs/home_assistant.md)
- `docs/` hardware notes ([docs/hardware.md](docs/hardware.md)), backup and restore, bring-up notes ([docs/bringup.md](docs/bringup.md)), security and development aids ([docs/security.md](docs/security.md)), design decisions and plans, feature list
- `kernel/` board device tree (`dts/`), mainline kernel patches (`patches/`), config fragment (`configs/`)
- `buildroot/` Buildroot external tree (`BR2_EXTERNAL`): root filesystem defconfig, init scripts, boot splash, small tools
- `assets/` logo and logotype (SVG), generated boot pictures
- `testinit/` minimal PID 1 used for the first bring-up test boots
- `tools/` build scripts, backup and restore, flashing over ADB or SSH, resource image pack/unpack, DTB dump, pstore log, boot splash renderer (`tools/splash/`), wake word benchmark
- `backup/` local firmware backups, git-ignored, **never published**

## How it is made

I build PanelOS with 15 years of experience as an SAP consultant on large enterprise systems. AI tools write much of the code, but the ideas, the design and a strict set of requirements (security, efficiency, measured performance) are my own, and every function goes through rigorous system and acceptance tests (SIT and UAT) on a real panel before a release. Nothing here is generated in one go: every part is specified, measured and tested.

## Warning

Flashing can brick the device. Make a full backup first (see [docs/restore.md](docs/restore.md)) and prove that you can restore it before you write anything. The partitions contain device-unique data (MAC addresses, keys, serial numbers): never share your backup images.

## License

AGPL-3.0-or-later (full text in LICENSE). The panel app uses the Slint toolkit under its GPLv3 option, which GPLv3 section 13 allows to be combined with AGPL-3.0 code. Third-party parts keep their own licences (Linux kernel GPL-2.0, Buildroot packages, DejaVu fonts, Material Design Icons, the microWakeWord models).
