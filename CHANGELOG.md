# Changelog

## 0.0.7 (in development)

### Connection
- When Sendspin music stalls (its buffer runs dry), the Wi-Fi roam check runs during the music and moves to an access point at least 10 dB stronger; a panel that fell to a farther access point during music no longer stays there until the music ends.

### Music Assistant
- Asking a panel for music by voice ("play Queen") plays on the player picked on its music page, or on the panel itself, as next, pause, stop and volume already did; a command that names a kind of device ("pause the TV") goes to Home Assistant's own handler.

### Home Assistant
- HACS and the Marketplace install the integration from `panelos.zip` on the release (2 MB) instead of the whole repository (about 90 MB, close to the Marketplace's 100 MB download limit).

## 0.0.6

**Upgrading from 0.0.5**: put the new UI bundle on the panel and update the integration, then restart Home Assistant ([docs/home_assistant.md](docs/home_assistant.md#upgrading-from-005)); the image and the web bundle stay as they are.

### Voice
- A ringing timer stops only on a stop word or a short stop phrase; a one-word fragment of a conversation no longer stops it. Speech over the ring counts from 8 dB over its echo.
- After the wake word, listening ends when nobody speaks within 5 s.
- The voice view keeps the microphone level and trace live while it shows; the comic answer waits until the thinking is over, and its bubbles stay layers over the swaying halftone.
- The integration's wake word rows show and set what the satellite really listens for, by name.

### Music Assistant
- The panel is a Sendspin player: Music Assistant finds it, pairs with it through Home Assistant and plays on it, with no setup. The music plays under every other sound and pauses while the assistant listens.
- One volume: Music Assistant's volume of the panel is the panel's own.
- An app restart tells Music Assistant first, so it connects again as soon as the app is back.
- The music page lists the panel itself first; the player picker opens at the top of its list every time.
- Music by voice with no setup: Home Assistant's own media commands (next, previous, pause, resume, volume, mute; in every language it has, in many variations) said to the panel act on the player chosen on its music page. Previous by voice goes to the previous track, not to the start of the current one.
- Music commands said to the panel can follow the player picked on its music page or always use the panel itself (Voice page: Music commands; default: the selected player). The panel reports its picked player; voice setups ask the integration (`hass.data["panelos"]["voice_player"]`).

### Connection
- The Wi-Fi roam check also finds access points that hide their network name (it asks for the network by name).
- A connection log on the persistent partition: Home Assistant sessions ending and coming back, roam checks with the access points they saw, moves and reconnects, with the local time ([docs/app.md](docs/app.md#connection-start-and-outages)).
- The integration keeps the panel's entities (all but the restart and reboot buttons) with their last state for 30 s through a short drop or an app restart instead of turning them unavailable.
- Illuminance is reported from a change of 5 lx, at most once in 10 s, and again every 10 minutes.

### Status bar, shade, sheets, pages
- The wake hint takes the whole room when there is no mark (with marks it ends just before them) and follows a mute at once; the speaker mark leaves when its tap brings the volume back; the bell counts unread notifications only.
- A pull that starts on a mark opens the shade. Leaving a page closes its sheets, the page dots never lie over a sheet, and a sideways drag on a sheet neither turns the page nor taps a row.
- A dropdown opens upwards when it would not fit below.
- The theme change crossfades the whole screen.
- After a ring, or a view shown for some seconds (`panelos.show_view`), the panel goes back to the page it was on.
- A reload or the dashboard coming on screen shows the loading screen first, never a white web page.
- Settings: the storage page sits under the status bar; a row's value takes the rest of the row.

### Pages
- A page out of sight stands still: clocks, nixie glow and news crawls run only while their page is on screen (before, a clock on another page kept every page redrawing 30 times a second); the synthwave scene's cars and palms are planned only while a scene is on screen (before, a synthwave timers page on the list redrew every page once a second).
- Timers: the synthwave look's running time no longer stutters every second (only the digits that change are drawn again), +1 min with two synthwave timers, the last minute in the alert colour, the ringing screen in the timer's colour, the timers page's own synthwave scene (or a random one every time); the new-timer view over running synthwave timers keeps the scene under a veil.
- Stopwatch: one lap button (the flag) and the lap list in the nixie look, which now runs at ~45 fps instead of ~9.
- Clocks: only the colon, the second hands and the seconds line move (30 fps); a still clock stays still (every clock face drew 60 frames a second whatever its style: the wandering face's corner spark read the animation clock in every frame). Calm clocks: when nobody has been near and nothing was asked of the panel for 10 minutes, every clock's colon turns gradually over 5 s from breathing into a plain step, bright one second and dim the next, and the seconds tick once a second (one frame a second instead of 30); a touch, an approach or a request from Home Assistant brings the breathing back, gradually over 2 s. Screensaver faces nixie, LCD and twin-bell from the integration.
- Music: transfer and group buttons in both themes, an info bubble in the player picker, the cover options in the music sheet, no thumbs on the sliders at rest, a vertical drag across the time bar no longer seeks, the half cassette's volume bar works, the turntable's volume fader jumps to a tap and follows a drag from where it is; a start after a pause no longer shows paused for a moment with the time jumping back, and the panel's own player shows playing while Music Assistant plays on it.
- Thermostat: quick presses count one by one, the remote's clock runs, the swing labels make room, the wall look's knob turns with the finger like a real knob (relative, with its detents) and sends the target when the finger lifts.
- Quick controls: channels and levers centred for any count, labels fall back to the whole name, a light's effects in a dropdown, the deck's lamp in the light's own colour; the flight deck has a label field in the integration.
- Calendar: the top-right chip names the calendar. To-do: one list picker (the chip).
- Notifications: the fridge note shows in the first window; the pager window steps on a swipe, and the pager's finish changes without an app restart.

### Integration page
- A sheet slides in once: an ⓘ or any other change inside it no longer replays its entrance in the narrow layout.
- The ⓘ buttons in the sound sheets open their bubbles.
- The layout editor draws the page's own background until the panel's picture of that page arrives.
- A new critical alert's test waits until something that starts the alert is chosen.
- The panel's live screen stops on its last picture after a minute without use of the page and goes on with the next use (only while the page is the active window; making it active counts as use).

## 0.0.5

**Upgrading from 0.0.4**: update the panel and the integration together ([docs/home_assistant.md](docs/home_assistant.md#upgrading-from-004)); the new interface runs from the UI bundle and the web bundle ([docs/app.md](docs/app.md#build-run-develop)).

### A new interface on the GPU

- **Drawn on the GPU**: Skia on OpenGL ES (Mesa Panfrost) through GBM, with page flips; the app and the pictures and fonts of its pages come as a UI bundle, and the image's own app takes over when the bundle is missing.
- **Pages side by side without an end**: a status bar with the wake hint and one centred group of marks (microphone off, speaker off, timers, the bell), page dots that stick together and tear during a swipe, the timer island with each running timer's ring, temporary pages for what the list lacks.
- **Quick shade in two steps**: brightness with automatic brightness, volume, tiles (microphone, theme, do not disturb, screen off), what runs now (timers, the newest notifications); the settings list behind the PanelOS mark (Wi-Fi, Home Assistant, display, voice, sound, storage, touch calibration, restart).
- **One control set** with the PanelOS page: segmented controls, dropdowns, switches, check boxes, sliders with a value bubble, buttons; every sheet follows the finger, and tall ones open at medium height and go up to full.
- **Light, dark and automatic**, changing smoothly; the PanelOS palette, type and mark everywhere.

### Pages and their looks

- **Custom page**: widgets on a 4 × 4 grid (clock, date, weather now, next event, forecast curve, hourly and daily forecast, music) over a living weather background (photos, drawn scenes, a line drawing, a plain ground) or a synthwave night drive with neon letters, palms in the wind, cars and a patrol car; several custom pages, each with its own background and widgets. Taps on the scenes do things there: a shooting star, a car in a lane, a car shoved aside, a city window lit or dark, trees shaking, a flag flapping.
- **Clock faces**: digital, big digits, round, square, minimal, numerals, wandering, split-flap, the time in words, nixie tubes on a walnut plinth, an LCD quartz watch and a twin-bell alarm clock; on the clock page, as a widget and on the screensaver.
- **Weather**: cards, or a living photo of the sky as it is now.
- **Music**: plain with the cover behind, full cover, a turntable (arm to seek, the record in the cover's colour, platter spinning up and coasting down), a cassette and half a cassette; player picker, moving music to another player, grouping.
- **Timers, alarms and a stopwatch**: plain, kitchen timer, hourglass, synthwave and flip clock looks; the stopwatch as a chronograph, a timing tower, an LCD sports watch or nixie tubes; the alarms as a twin-bell clock, a sunrise, a clock radio or a 24-hour dial; a sheet per timer (+1 min, pause, cancel), alarms with days, label and sound.
- **Thermostat**: a dial, a remote control, a wall thermostat and a neon one; room by room, with fan, swing and presets.
- **Quick controls**: tiles with a details sheet, a mixing desk with faders and meters, a flight deck with toggle levers and lamps.
- **Cameras**: one big or all of them; the doorbell wakes the panel with the visitor on screen and, with a lock, hold to open.
- **Calendar**: agenda, day, three days, week and month, a departure board, a tear-off calendar and a wall calendar written in pen; event details from every view.
- **To-do lists**: plain, a shop receipt, a terminal, label tapes and a notepad in handwriting; adding with a description and a due date, ticking and unticking, several lists.
- **Notifications**: a list, fridge notes, a pager and the evening news.
- **Every look moves in its own way**: digits that roll, fill or flip, nixie afterglow, LCD segments that lag, the hourglass turning over, the turntable's arm, a receipt that feeds, new notifications sliding, slapping, buzzing or breaking in, a lamp test, and many more small motions across every page.

### Over the pages

- **New notifications** in a window in the notifications page's look: several in one window, OK marks read, Dismiss removes; kept for a while or until answered.
- **Status screens and critical alerts** from the integration's rules: in their own colour, folded into a strip; a critical alert sounds until held.
- **Do not disturb** with what it lets through; the **screensaver** inside its window (dark room, times, sunset to sunrise, always), never while a timer runs.
- **Voice view** in four looks (plain, terminal, comic, oscilloscope): listening, listening for more, not understood, thinking, the answer typed as it is spoken; the oscilloscope draws the sound itself.
- **Keyboard** with accented letters on a long press, Latin and Cyrillic letters, a capital at the start.

### Voice, timers and sound

- **Timers live in Home Assistant**: started and changed on the panel or by voice from any device of the house (the panel named by its room), the same timers everywhere.
- **A spoken stop over a ringing timer or alarm**: the panel hears its own Stop and the wake words and checks short phrases with speech recognition, outside any conversation.
- **Sounds**: timers, alarms and critical alerts ring with a chosen sound (the panel's own, uploaded ones, or music from Music Assistant for alarms, rising from quiet).
- **The panel is a media player** in Home Assistant (music and announcements streamed to its speaker).

### Home Assistant

- **The PanelOS page rebuilt**: every panel with its live screen beside its settings; pages in swipe order with each look shown as the panel draws it, options per page, a widget editor with sizes and styles, alerts and status screens shared by all panels, voice, display, device, copy settings to other panels, add a panel; wide, medium and phone layouts, explanations behind (i) buttons.
- **PanelOS notifications**: a notify entity per panel and the `panelos.notify` action (title, message, icon, level, a picture, up to three buttons that fire `panelos_notification_action`, until answered).
- **Voice tools** for agents that use the Assist API: start, cancel and list a panel's timers by its room.
- **Sounds and alarms** on the page: a sound picker with listening, uploads (MP3, WAV, OGG, FLAC), the panel's alarms.
- Own critical alerts started by a device or an automation (`panelos.critical_alert`, `panelos.clear_critical_alert`); status screens started by automations.

### Connection

- Keep-alive with Home Assistant every second and a link watch that reconnects a link that says connected but carries nothing.
- The last settings from the integration are kept on the panel and used at start.
- Roaming also judges the link's quality and avoids a poor access point for two hours; Wi-Fi power save always off.

## 0.0.4

**Upgrading from 0.0.3**: the integration is now `panelos` instead of `nspanel_pro`. Write the new image to the panel, install PanelOS in Home Assistant, remove the old NSPanel Pro integration and its `custom_components/nspanel_pro` folder, restart Home Assistant and add PanelOS ([docs/home_assistant.md](docs/home_assistant.md#upgrading-from-003)).

### Home Assistant

- **The panel is a device of its own**: the integration is now `panelos` ("PanelOS") and adds the panel as a device (maker PanelOS, model NSPanel Pro 86 Gen1) with the display, the settings and the sensors. The panel reports them over its own Home Assistant connection (`panelos/report`) and takes the commands back on its subscription. The ESPHome device keeps the voice satellite (volume, microphone, wake word sensors), is named "NSPanel Pro Voice" and is shown as connected via the panel's device. Entities that were on the ESPHome device keep their entity ids, history and settings.
- Integration options as a menu: Dashboard mode (dashboard, header, sidebar), Assistant mode, Sensor reporting.
- **PanelOS page in the sidebar** (administrators), in the PanelOS look: all settings of a panel beside a live picture of its screen, the card resources it loads, its device facts; the sidebar icon is the PanelOS mark.

### Dashboard mode

- **Smooth scrolling**: while the dashboard is all there is on the screen, the browser draws straight into the display's buffers on the GPU and the display flips to them; no read-back, no copy, no drawing on the CPU. A frame reaches the screen in about 20 ms instead of 50, swipes run at 38–56 frames per second instead of 14–25.
- **Faster loading**: card resources the dashboard shown does not need are not loaded on the panel, worked out again whenever the dashboard changes (integration option, on by default; resources can be kept); about a quarter less load time on a dashboard with 35 resources.
- **Loading screen**: the PanelOS mark writes itself in Morse while a dashboard loads, covering Home Assistant's start-up, and the dashboard fades in once its cards and icons are there; it can be switched off (integration option).
- **Size**: dashboards can be drawn smaller or larger (50–150 %).
- **Pull to refresh**: pulling the dashboard down reloads it, as in a phone's browser, when the content is at its top; the top edge stays the swipe to the settings.
- Noto Sans as a second font in the web view: card texts no longer show boxes for characters such as Mushroom's separator.
- Dashboard mode is a secondary function next to the assistant; the docs say what a dashboard for the panel should look like (light and simple).
- **Home Assistant dashboards in a web view**: WPE WebKit on the GPU (Panfrost), logged in as the panel's own user like the Home Assistant apps, custom cards included; header and sidebar can be hidden. A swipe down from the top edge opens the settings. The browser is a separate SquashFS bundle on the persistent partition; without it the panel shows its native dashboard.

### Brand

- PanelOS has its own look: the mark (P in Morse, the red dots and the dashes), the logo, the Morse pattern, guidelines and a kit with icons, logos, a start-up film and its sound (`branding/`).

### Installation

- **Removing the stock Android** (optional install step 14, `tools/remove_android.sh`, over Wi-Fi, panel closed): PanelOS is copied to the boot partition, so a normal boot and the test-boot safety net start PanelOS instead of Android, and the old userdata partition (about 3.5 GiB) becomes `/mnt/data` for larger content. The way back writes the stock partitions from the backup (restore guide, case D). Tried on one panel (the way back not yet).
- e2fsprogs in the image (`mkfs.ext4`, `e2fsck`); `tools/build_image.sh` warns when the image would not fit the boot partition.

### Fixes

- Touch: the touch controller sends frames with a stale status byte while a finger rests and at the edges; the driver dropped them, so corners seemed insensitive and a long press did not register. The driver now reports them as the touch they are, paces them and ignores the repeat the controller sends after a release. Measured: 36 of 36 points at the first touch, corners included, no ghost touches (docs/hardware.md).
- Touch test view for development (`/tmp/nspanel-touch-test`): nine points, each touch logged with its distance from its point.

## 0.0.3

### Security

- **Encrypted ESPHome API**: Home Assistant gives the panel a key right after adding it (as with ESPHome devices) and connects encrypted from then on; plaintext and other keys are refused. Only Home Assistant's address may connect (`api_allow` in `app.json` adds more).
- The API checks frame sizes, keeps at most three connections, and a malformed message ends only its own connection. A voice run streams the microphones for at most 30 s; conversation events only count while the panel's own run is going on.
- Audio is fetched only from Home Assistant's host, without redirects, with limits on size, format and length; one announcement at a time. The login token goes only to Home Assistant itself; logging out revokes it.
- mDNS answers only the panel's own network; the login web server stops after pairing; camera images are checked for size; only installed wake word models can be activated.
- SSH: root logs in with the key only (password locked); the host key is kept on the persistent partition, and the guides check it instead of ignoring it.
- Kernel and network settings: kernel addresses hidden from unprivileged users, no ICMP redirects or source routing.
- Build: kernel and Buildroot at fixed commits, wake word models checked against SHA-256 sums.
- Integration: `show_view` and `show_message` for administrators and automations only, message lengths limited.
- `docs/security.md`: what is protected, how, and the development aids that stay on for now.

### Voice

- **Long answers without gaps**: a streamed answer starts once 0.8 s of audio is in (at most 1.5 s), so a wait of up to about 0.8 s in Home Assistant's stream no longer cuts the speech; the answer starts up to 0.8 s later.
- The conversation stays on the screen until the answer has been spoken (Home Assistant ends the run when the speech starts).

### First start and discovery

- **Setup screen on the panel**: Home Assistant found on the network (mDNS) or its address typed on a keypad; the panel checks that Home Assistant answers there, saves it and shows the pairing QR code. No SSH needed for the first setup.
- **mDNS announcement** of the ESPHome API (`_esphomelib._tcp`), like any ESPHome device: Home Assistant discovers the panel and keeps its address up to date (inside one network segment). Built into the app, no system service.

### Power cuts and outages

- The panel keeps trying for good when Home Assistant or the network is away, and comes back by itself without a restart: retries every 0.5 s right after the start, then every 5 s without network and every 10 s at most while Home Assistant does not answer.
- A vanished Home Assistant is noticed within about 30 s; failures are logged once per cause instead of on every try.
- Clear status: "Waiting for the network…" or "Home Assistant not reachable, retrying…".
- No 1970 time on the screen: `--:--` until NTP or Home Assistant has set the clock.
- NTP starts whenever Wi-Fi gets an address, however late, and restarts if it exits.
- Assistant mode: the media card stays hidden until its first state arrives.

### Wi-Fi

- **Back to the nearer access point**: wpa_supplicant changes the access point only when the link breaks, so after an access point outage the panel stayed on a farther one for hours. Now, about every 30 minutes and only while the panel is idle (no voice run, no sound, no ringing timer), it scans and moves to an access point of the same network that is at least 10 dB stronger. There is no scan at -49 dBm or better (the thresholds of ESPHome's `wifi.roam`). The move is one-shot: the panel is never tied to one access point. One log line per check.
- wpa_supplicant logs connections, disconnections and access point changes to the system log (syslog support turned on in Buildroot, `-s`). `tools/build_image.sh` rebuilds wpa_supplicant once in an older Buildroot output directory.

### Display

- **Light and dark theme by the light sensor** from the first start, the setup screen included: dark below 10 lx, light above 30 lx, a change only after 10 s, so it does not flicker at dusk.
- **Brightness scale** even in perceived brightness, from 0.8 % (night) to 100 %, for automatic and manual brightness; in the evening (about 50 lx) the panel is now noticeably dimmer than before.
- **Smooth brightness changes**: fades every 20 ms instead of steps twice a second; the backlight has 1000 levels instead of 256.
- **Wake on approach works**, from about 40–45 cm: the proximity sensor runs at its most sensitive gain, 11.84 ms integration time and the IR LED at full power (at the driver's defaults it read 0). The threshold is the signal of a hand at 45 cm from a distance curve measured with a ruler, or 5 times the measured noise if that is more; a slow approach and a hand held just below the threshold do not raise it. The Presence entity uses the same detection.

### Home Assistant

- **Fewer database writes**: illuminance, SoC temperature and Wi-Fi signal are sent only after a set change instead of every minute's reading: light by 15 % of the last value sent (at least 5 lx), temperature by 1 °C and signal by 3 dB on five-minute averages. Each step can be set finer or coarser in the integration's options (**Sensor reporting**), with an explanation of what it means.
- The integration's options open with a choice between **Assistant mode** and **Sensor reporting**.

### Boot

- **Packed kernel**: the boot image is about 30.5 MB instead of 41 MB and fits the 32 MiB `boot` partition, the base for A/B updates. The chain loader unpacks it with caches on in about 1 s; the boot time stays the same.

## 0.0.2

### Boot

- **Super fast boot**: about 12 s from power on to the dashboard (stock firmware: about 53 s). The stock bootloader phase dropped from about 45 s to about 2.5 s.
- **PanelOS logo in the bootloader**: the stock U-Boot shows `assets/bootlogo.bmp` (the first frame of the boot animation) at the brightness of the animation.
- **Chain loader** in front of the kernel (`tools/chainload/`, packed by `tools/arm64_chain_pack.py`): the stock U-Boot gets the stock device tree, the kernel its own one, with the memory layout, command line, initramfs location and serial number carried over.
- **Watchdog**: the chain loader starts the SoC watchdog and the kernel feeds it; a boot that hangs early resets the panel by itself.
- The backlight starts at 180 (bootloader, kernel and boot animation alike), no jump in brightness.

### Panel app

- The UI takes over the display only when it has current content: the clock is set and the first Home Assistant states are in. The boot animation's light band shows the wait.
- The clock comes from Home Assistant until NTP has set it, so it is right on the first screen, also on networks without internet access.
- While the network is not up yet, the Home Assistant connection is retried every 0.5 s instead of with a growing delay.
- Voice satellite: new sensors Wake level, Wake SNR and Wake score, sent with every wake word before the voice run, so Home Assistant can let the satellite that heard best answer.

### Install and build

- The build now takes the stock device tree from a panel backup (`resource` partition, `tools/make_uboot_dtb.sh`); the install guide backs up the panel before the build (steps 5–7 reordered).

### Documentation

- `docs/hardware.md`: the Zigbee module (EFR32MG21, EmberZNet 6.10.1 NCP, EZSP 8, Gecko Bootloader 1.12 with serial XMODEM upload) and the SoC console (UART2 TX on GPIO1_D2 and GPIO2_B4, not broken out to any pad).
- `docs/bringup.md`: chain loader, watchdog and boot times; `docs/decisions.md`: the stock-device-tree decision.

## 0.0.1

First version.

### System

- Mainline Linux kernel (7.3) trimmed to the board (17.8 MB Image) and a Buildroot root filesystem as initramfs, in the panel's recovery partition; the stock firmware and U-Boot stay untouched.
- Kernel patches: a panel descriptor for the Sitronix ST7701S display and CST226SE support in the Hynitron touch driver. Wi-Fi with rtw88 (RTL8723DS), audio through the RK809 codec, the PDM microphones, light and proximity sensor (STK3310).
- Default boot into PanelOS through the stock U-Boot's bootloader message (`nspanel-bootmode open | android | status`); one-shot test boots that fall back to the stock firmware without network (`S41testboot`).
- Settings, Wi-Fi configuration and the Home Assistant login on a persistent partition; Wi-Fi recovery by rebinding the SDIO controller.
- Boot animation from the first second of user space (`tools/splash/`, `nspanel-splash`), with a light band until Wi-Fi is up; the UI comes as soon as Wi-Fi is up.

### Panel app (Rust + Slint, drawn straight to DRM/KMS)

- Native Home Assistant dashboard from a dashboard chosen for the panel: tiles, entities, headings, thermostat, media control, camera, weather, glance and gauge cards; tabs for several views; live states for exactly the dashboard's entities; dashboard edits appear at once.
- Control by tap; details layers for lights (brightness, colour temperature, colour), shutters, climate and media.
- Camera snapshots through Home Assistant, about 1 frame/s, full screen on tap.
- Pairing by QR code (Home Assistant's own login, OAuth), no token typing.
- Themes (dark, light, auto), settings screen, screensaver (clock or screen off), automatic brightness from the light sensor, wake on approach.
- Assistant mode: smart-display home screen with clock, weather and forecast, media card and quick tiles; weather, media, conversation, message and camera layers.

### Voice satellite (ESPHome native API)

- On-device wake words (microWakeWord: Okay Nabu, Hey Jarvis, Hey Mycroft, Alexa and experimental ones), selectable from Home Assistant; an internal "stop" model while the panel speaks or a timer rings.
- Assist pipelines, echo cancellation (WebRTC AEC3) while the speaker plays, so the wake word works during an answer too.
- Follow-up listening; an empty follow-up ends after 5 s.
- Timers with a full-screen alarm (stopped by "stop" or a short sentence without the wake word), announcements, volume and microphone mute.

### Home Assistant

- The panel is an ESPHome device for Home Assistant's built-in integration: settings and sensors as entities (screen, dashboard, theme, brightness, volume, illuminance, SoC temperature, Wi-Fi signal, presence and more).
- Optional integration `custom_components/nspanel_pro` (Home Assistant Marketplace, HACS or manual): assistant mode settings, `show_view` and `show_message` actions.

### Install and tools

- Install over Wi-Fi ADB from the rooted stock firmware, no opening the case (`tools/flash_recovery_adb.sh`); updates from a running PanelOS over SSH (`tools/flash_recovery_ssh.sh`).
- Backup of all stock partitions (`tools/backup_partitions.sh`) and restore paths, including the USB rescue mode (`docs/restore.md`).
- Step-by-step install guide for Windows (WSL), Linux and Mac (`docs/install.md`).
