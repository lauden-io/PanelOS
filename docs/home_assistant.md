# Home Assistant integration: `panelos`

Custom integration for panels running PanelOS (`custom_components/panelos` in this repository). With it a panel is a device of its own in Home Assistant:

- **The panel's device** (maker PanelOS, model for example NSPanel Pro 86 Gen1) with the display, the settings and the sensors as entities (see [Entities](#entities)).
- **Dashboard mode settings** (integration options): which Home Assistant dashboard the panel's web view shows, with or without its header and sidebar.
- **Assistant mode settings** (integration options): weather entity, media player, full-screen conversation on/off.
- **Sensor reporting** (integration options): how much the illuminance, SoC temperature and Wi-Fi signal must change before the panel sends a new value (see below).
- **The PanelOS page** in the sidebar (administrators): every setting of a panel in one place, beside a live picture of its screen (see [The PanelOS page](#the-panelos-page)).
- **Critical alerts and status screens**, shared by every panel (each with its own panels on or off): the integration follows their sensors and shows them on the panels (see [Alerts and status screens](#alerts-and-status-screens)).
- **Actions** that put something on a panel's screen (administrators and automations only):
  - `panelos.show_view`: `view` = `home` (the start page, the first in the page list), `dashboard`, `weather`, `media` or `camera` (with `entity_id: camera.…` the cameras page that has that camera, with it shown big; a camera no cameras page has comes alone on a page of its own), `seconds` (default 30; after the seconds the panel goes back to the page it was on, unless someone paged meanwhile; 0 = until someone touches the screen: the view stays and the screen stays on until the first touch, then the panel is as usual), `panel_id` (empty = every panel). A view whose page the panel lacks comes as a page of its own, gone once left. A page view comes over the dashboard of dashboard mode too, and the dashboard (on a panel with its web view) over the pages of assistant mode; the panel's own mode comes back when the seconds run out, or with 0 once nobody has used the panel for the screensaver's time after the touch (two minutes when the screensaver never starts).
  - `panelos.notify`: `panel_id` (one or several; none = every panel), `title` (up to 100 characters), `message` (up to 1000), `icon` (`mdi:…`), `level` (`info` / `warning`), `picture` (a camera entity or a JPEG address), `buttons` (up to three; a press fires `panelos_notification_action` with `notification_id`, `action`, `index` and `panel_id`), `until_answered`. Each panel also has a `notify` entity for `notify.send_message`.
  - `panelos.show_status` / `panelos.clear_status`: `screen` = the name in automations (or the ID) of a status screen whose trigger is "Automation", as the PanelOS page shows it.
  - `panelos.critical_alert` / `panelos.clear_critical_alert`: `alert` = the ID of an own critical alert whose trigger is "Automation" (as the PanelOS page shows it).
  `panelos.show_view` fails with an error when the panel it names is not connected (without `panel_id`: when no panel is); `panelos.notify` keeps the message for a panel that is away and delivers it when the panel is back.

The voice satellite stays on Home Assistant's built-in ESPHome integration: the panel is an ESPHome device for it, named after the panel with "Voice" added, and shown as connected via the panel's device. Without this integration the panel still works (its default pages or Home Assistant's dashboard, voice through ESPHome), but its display and settings have no entities in Home Assistant.

## Install

Pick the way that matches your Home Assistant. You need an administrator user in Home Assistant.

### A. Marketplace (Home Assistant 2026.11 and later)

The Marketplace is built into Home Assistant since 2026.11 (it replaces HACS).

1. In Home Assistant open **Settings → Marketplace**.
2. The first time, connect it to GitHub when it asks: it shows a code, you open the GitHub page it links, sign in to GitHub and enter the code. A free GitHub account is enough.
3. Select **Add from link** and enter the repository URL: `https://github.com/lauden-io/PanelOS`
4. Open **PanelOS** in the list and select **Download**. (The repository must be public for this; the Marketplace cannot read private repositories.)
5. Restart Home Assistant: **Settings → System**, the power icon at the top right, **Restart Home Assistant**.
6. Continue with [Set up](#set-up).

### B. HACS (Home Assistant before 2026.11, with HACS installed)

[![Open your Home Assistant instance and open this repository in HACS.](https://my.home-assistant.io/badges/hacs_repository.svg)](https://my.home-assistant.io/redirect/hacs_repository/?owner=lauden-io&repository=PanelOS&category=integration)

The button opens steps 1–3 in your Home Assistant.

1. Open **HACS** in the sidebar.
2. Select the **three dots** at the top right → **Custom repositories**.
3. Repository: `https://github.com/lauden-io/PanelOS`, Type: **Integration** → **Add**, then close the window.
4. Search for **PanelOS** in HACS, open it and select **Download**, then **Download** again.
5. Restart Home Assistant: **Settings → System**, the power icon at the top right, **Restart Home Assistant**.
6. Continue with [Set up](#set-up).

### C. By hand (any Home Assistant)

1. On the PanelOS GitHub page select **Code → Download ZIP** and unpack the file on your computer.
2. Make the Home Assistant configuration folder reachable from the computer. The simplest way: **Settings → Apps → App store → Samba share** → Install. On its Configuration tab set a user name and password, save, then **Start** it.
3. Open the configuration folder:
   - Windows: in File Explorer type `\\<Home Assistant IP address>\config` into the address bar and log in with the Samba user name and password.
   - Mac: Finder → Go → Connect to Server → `smb://<Home Assistant IP address>/config`.
4. In the `config` folder, create a folder named `custom_components` if it is not there yet.
5. From the unpacked ZIP, copy the folder `custom_components/panelos` into that `custom_components` folder. The result is `config/custom_components/panelos`, with the `__init__.py` and `manifest.json` files directly inside.
6. Restart Home Assistant: **Settings → System**, the power icon at the top right, **Restart Home Assistant**.

## Upgrading from 0.0.7

1. Build the new UI bundle and put it on the panel (`tools/build_ui_bundle.sh`, then `tools/install_bundles.sh <panel IP> ui`; see the [install guide](install.md#updating-later), "Updating later"); the image and the web bundle stay as they are.
2. Update the integration (Marketplace, HACS or a copy of the new `custom_components/panelos` folder) and restart Home Assistant. The settings stay.

## Upgrading from 0.0.6

1. Build the new web bundle and the new UI bundle and put them on the panel (`tools/build_web.sh`, `tools/build_ui_bundle.sh`, then `tools/install_bundles.sh <panel IP>`; see the [install guide](install.md#updating-later), "Updating later"); the image stays as it is.
2. Update the integration (Marketplace, HACS or a copy of the new `custom_components/panelos` folder) and restart Home Assistant. The settings stay; the panel's Home Assistant user is kept on its first connection (Device → About).

## Upgrading from 0.0.5

1. Build the new UI bundle and put it on the panel (`tools/build_ui_bundle.sh`, then `tools/install_bundles.sh <panel IP> ui`; see the [install guide](install.md#updating-later), "Updating later"); the image and the web bundle stay as they are.
2. Update the integration (Marketplace, HACS or a copy of the new `custom_components/panelos` folder) and restart Home Assistant. The settings stay.

Music Assistant then finds the panel as a Sendspin player and pairs with it through Home Assistant by itself (see [docs/app.md](app.md#voice-satellite)).

## Upgrading from 0.0.4

Update the panel and the integration together: the new interface uses the integration's newer requests (timers through Home Assistant, PanelOS notifications, page settings from the panel).

1. Build the new image, web bundle and UI bundle, write the image to the panel and put the bundles on it ([install guide](install.md#updating-later), "Updating later", with the web bundle).
2. Copy the new `custom_components/panelos` folder to Home Assistant and restart it. The settings stay; the PanelOS page loads its new version by itself.

## Upgrading from 0.0.3

Up to PanelOS 0.0.3 the integration was called NSPanel Pro (`nspanel_pro`); since 0.0.4 it is PanelOS (`panelos`), and the panel is a device of its own.

1. Write the new image to the panel (install guide, "Updating later").
2. Install PanelOS as above, then remove the old **NSPanel Pro** integration (**Settings → Devices & services**, its entry, **Delete**) and the folder `custom_components/nspanel_pro`.
3. Restart Home Assistant and add PanelOS ([Set up](#set-up)); set its options again (weather, media player, dashboard).

The panel's settings and sensors move from its ESPHome device to the PanelOS device and keep their entity ids, history and settings; the ESPHome device keeps the voice satellite.

## Set up

1. **Settings → Devices & services → Add integration**, search for **PanelOS** and select it.
2. Panel ID: the name the panel was given at installation (the panel's settings show it under About), `nspanel-pro` for a panel without one. Then choose the weather entity and the media player.
3. The panel's device appears with its entities; they become available as soon as the panel connects. Switch the panel to the mode you want with its **Display mode** select (or on the PanelOS page: Pages, the Dashboard card, **The panel shows**).

Options can be changed any time (**Configure** on the integration entry, then **Dashboard mode**, **Assistant mode** or **Sensor reporting**); the panel follows at once.

## Several panels

Each panel is one entry of this integration, found by its Panel ID: the short name given to the panel at installation ([install.md](install.md), steps 5 and 9: letters a-z, digits and hyphens (not at either end), at most 31 characters, for example `kitchen`). The panel keeps the name in `/mnt/persist/nspanel/panel_name` and uses it everywhere: as `panel_id` towards this integration, as its ESPHome device name and mDNS name (`kitchen.local`), and in its device names (NSPanel Pro (kitchen), NSPanel Pro Voice (kitchen), the Music Assistant player NSPanel Pro (kitchen)). A panel without a stored name is `nspanel-pro`, its devices are NSPanel Pro and NSPanel Pro Voice, and its devices and entities stay as they are.

- Give every panel its own name, one that no other ESPHome device in Home Assistant uses: the panel's ESPHome device is found by this name.
- Add a further panel on the PanelOS page (**Add a panel**): a panel that already asks Home Assistant for its settings is filled in. **Settings → Devices & services → Add integration → PanelOS** works too.
- A panel with a name only ever takes its own entry. A panel without one (`nspanel-pro`) also takes a single entry set up with another Panel ID.
- Every panel has its own entities, user, timers, alarms, notifications and voice satellite; the voice tools pick a panel by its room or name.
- After replacing a panel's hardware, delete the old panel's ESPHome entry (or accept Home Assistant's repair that moves it to the new device): the panel's user then follows the new panel by itself. While the old entry is there, it stays the panel's.
- For development, `tools/fake_panel.py` talks to Home Assistant as a panel of another name (subscription, device report, keep-alive, commands; no ESPHome device, so no voice satellite and no timers), so two panels can be tried with one.

### Renaming a panel

The name is the panel's identity in Home Assistant, so a new name means a new PanelOS entry (the device's name in Home Assistant is not the Panel ID: the PanelOS page changes it, with the area, under Device, Room):

1. On the PanelOS page, **Add a panel** with the new name; under **Start with** choose **A copy of another panel** and the panel's current entry.
2. Delete the old entry: **Settings → Devices & services → PanelOS**, the old entry, **Delete**.
3. Write the new name on the panel and restart it (the command of [install.md](install.md) step 9, with `NAME` set to the new name). To go back to no name (`nspanel-pro`), remove the file instead: `rm /mnt/persist/nspanel/panel_name`.

The PanelOS device then comes with new entities (automations that use the old entity ids need the new ones). The ESPHome device keeps its entities and its key: Home Assistant knows it by its MAC address and takes the new name by itself.

## Entities

On the panel's device (this integration):

| Entity | Kind | What it does |
|---|---|---|
| Screen | switch | Off starts the screensaver now, on wakes the panel |
| Dashboard | select | The dashboards the panel's Home Assistant user may open |
| Theme | select | Dark, Light or Auto (by the light in the room, the sun or set times, as set on the PanelOS page) |
| Screensaver after | select | Never, 30 s … 30 min |
| Screensaver style | select | Clock or Screen off |
| Display mode | select | Dashboard or Assistant |
| Time format | select | 24-hour or 12-hour, for every clock time the panel shows (also in the PanelOS page's Display section and the panel's settings → Display; a change anywhere applies everywhere) |
| Brightness | number | Backlight when automatic brightness is off, 1–100 % |
| Automatic brightness | switch | Backlight follows the light in the room |
| Wake on approach | switch | The proximity sensor wakes the screen |
| Child lock | switch | Touches do nothing until the lock is held on the panel for 3 s; it locks again after a minute without a touch |
| Rain on the glass | switch | Drops on the glass over the photo backgrounds when it rains (on by default) |
| Lightning | switch | Flashes and bolts in the storm scenes (on by default) |
| Illuminance | sensor | Light at the panel, lx |
| Presence | binary sensor | Someone in front of the panel (proximity sensor) |
| SoC temperature, Wi-Fi signal, IP address, App version | sensors | Diagnostics |
| Restart app, Reboot | buttons | |

The entities keep their last state for 30 s after the panel went away (a Wi-Fi hiccup, the app restarting), and turn unavailable only when it is still away then. The buttons are available only while the panel is connected. A reload of the integration keeps the last states and the 30 s as well.

On the voice device (ESPHome): the Assist satellite, the media player (see [The panel's media player](#the-panels-media-player)), Volume, Mute microphone, and the Wake level, Wake SNR and Wake score sensors (sent with every wake word, see [app.md](app.md#esphome-native-api)), Panel user (diagnostic: the id of the Home Assistant user the panel is logged in as; only that user and administrators may act as the panel, see "How it talks to the panel"), plus the selects Home Assistant creates for satellites (Assistant, Wake word, Finished speaking detection). The two wake word selects follow what the panel listens for: Home Assistant itself takes a satellite's wake word into them only when it has exactly one, so the integration sets them to the panel's words whenever they differ (also after a change on the panel or on the PanelOS page).

Up to PanelOS 0.0.3 the panel's display, settings and sensors were on the ESPHome device. The integration takes these entities over with the same entity ids, history and settings. If the panel still runs an older PanelOS, they stay on the ESPHome device until it is updated, then move.

## The PanelOS page

**PanelOS** in the sidebar (with the PanelOS mark as its icon) is a page of its own in the PanelOS look: Space Grotesk headings, Noto Sans text, Signal red only for what is lit or active, Graphite or Paper ground following Home Assistant's dark or light mode. It holds every panel: on a wide screen a menu on the left (the PanelOS logo, a panel switcher, the sections, All panels, Add a panel) with the selected panel's screen beside it as a live picture; on a medium screen a top bar with section pills; on a phone a bottom bar and a small floating screen. Editors open as drawers (wide) or sheets (phone); less used settings are behind **More options**, explanations behind the (i) buttons.

- **Overview**: the panel at a glance: its live values, the alerts and status screens on now, and Home Assistant's notifications, each with **Dismiss**.
- **Pages**: the pages in swipe order (the first is the start page), each with its look and its options; each page's menu moves it up or down, copies it to other panels, duplicates it or removes it (the start page stays). The options: a custom page's background (synthwave: its cars and how busy they are; drawn, line and plain: light, dark or as the panel's theme; photo, drawn and line: the real sun and moon and lightning, the photos also rain on the glass) and widgets on its 4 × 4 grid, the weather details page's background and weather widgets (the same editor, with weather now, details, the forecast curve and the hourly and daily forecasts to choose from), the player and the turntable's wood, the timers page's functions, colours and synthwave scene (one of the four, or a random one each time the page opens), the quick controls' entities (**Fill from the room** adds the lights, shutters, climate, switches, fans and scenes of the panel's area), their short labels (the mixing desk's tapes, the flight deck's plates) and the flight deck's group frames (automatic by kind, off: one frame with the panel's room, or custom: a group starts at the tiles chosen, each with its name), the cameras' fit, start view, doorbell and replies at the door (the speaker at the door, up to four replies of one line on the panel, their language), the calendars shown on the panel (also chosen with the calendar page's chip) and those written on the wall calendar, the pager's finish (also set in the pager's own menu on the panel), the clock page's face (any of the screensaver's eleven, nixie, LCD quartz and twin-bell included) and its parts. Every page type can be added any number of times (up to the panel's twelve pages), each page with its own settings: ten music pages, each on its own player, two quick controls pages with tiles of their own, a cameras page per door with its own doorbell, calendar and to-do pages each with their own calendars and lists, clock and timers pages each in their own look. The page list names what each page shows (its player, its first tiles, cameras, calendars or lists, its look), so pages of one type are told apart; a page can be hidden without losing its settings. A quick controls page takes eight tiles (the panel's grid, the mixing desk and the flight deck show eight): the Tiles card offers "Add" up to eight, more go on another quick controls page (a page with more tiles from an earlier version is split into pages of eight when the integration starts, each tile with its label and group), and a tile is moved by dragging its handle, with the mouse or a finger, as the pages are in the page list. The widget picker shows each widget as the panel draws it, with live content from Home Assistant: the running clock and today's date, the page's weather now and its forecasts, the next event of the page's calendars and the music of its player; a widget without data shows the panel's own picture with demo content.
- **Alerts**: critical alerts and status screens (shared by every panel, see below), with a test on one panel; the panel's do not disturb (off, on or on a schedule, and what comes through) and how new notifications show (a strip at the top, a window or only in the list; sound, until answered, how long).
- The panel's live screen next to every view runs while the page is used: after a minute without use (mouse, scrolling, keys, touch) it stops on its last picture ("Paused") and goes on with the next use. Use counts only while the page is the active window; making the window active again counts as use.
- **Voice** (the conversation's look on the screen; **Music commands**: whether music asked for by voice on the panel goes to the player picked on its music page, the default, or always to the panel itself; the voice satellite's own settings), **Display** (theme and what switches Auto: the light in the room, sunset and sunrise, or set times; brightness; time format; the screensaver: after how long, when, its style and its clock's face, parts, letters, width, wandering speed and colour; waking: on or off, the distance near / medium / far and its calibration; the child lock) and **Device** (the panel's details and its Home Assistant user, the device's name and area, Wi-Fi roaming on or off, sensor reporting, touch calibration, restart, reboot, and **Remove** under More options).
- **All panels**: what each panel can do, and **Copy settings** to other panels (each part copied, adapted to what the target can do, or skipped, with the reason).
- **Add a panel** starts the integration's setup for another panel, with its defaults or a copy of an existing panel.

A weather details page without a background of its own has the first custom page's, and without widgets of its own it shows the weather now, the details (wind, humidity, chance of rain, sunrise and sunset), the hourly and the daily forecast, one row each; its widgets open nothing on a tap. The synthwave cars and the real sun and moon follow the first custom page's settings, and the panel shows one synthwave scene: the first page with a synthwave background chooses it.

The clock widget's style picker greys a style that does not fit the widget's size, and resizing a widget stops at its smallest size and says why (nixie 3 × 1, or 4 × 1 with seconds; LCD quartz 2 × 1; twin-bell 2 × 2). The timers page has a picker for the stopwatch's and the alarms' own looks.

Widgets besides the clock, the date, the weather, the next event, the forecasts and the music: **Sensor** (one sensor's value with its unit, its icon in the widget's colour and its name; the sensor is chosen in the widget's settings), **Timer** (the running timer that ends soonest, two side by side from four cells wide: its ring in the timer's colour, the time left and its name; gone while no timer runs; a tap opens the timers) and **People** (the people's heads: each person's profile picture in Home Assistant, or their initials in their colour without one or with **Profile pictures** off (on by default); with **The ones away too** on, the ones away or unknown show too, fainter inside a grey ring; the people it shows are ticked in its settings, here and on the panel: all ticked is everyone, the ones added to Home Assistant later too, and at least one stays; the heads take as many rows as make them biggest in the widget's size, so a 1 × 1 holds nine). Widgets may lie on each other; the later one lies on top, and **Forward** / **Back** (shown for a selected widget that lies on another) change that.

**Allow editing on the panel** (each custom page's Layout card, one switch for the panel, off by default): a finger resting on a custom page for 0.7 s opens a lock; holding the lock for a second opens the edit mode on the panel. There widgets are moved (drag), resized (drag the thick corner), removed (−), set (a tap opens the widget's settings), put forward or back, and added (+, then a tap on a free cell); the synthwave background's palette sets its scene, the neon letters and the cars. Every change on the panel is sent at once and kept here as the page's widgets, so this editor shows it too. With the switch off, the integration refuses widgets sent from the panel (`not_allowed`), whoever sends them.

Every look, background, voice look and screensaver clock face is shown as the panel itself draws it. The pictures come from a panel running the app with demo content (neutral weather, calendars, lists and a player with its own cover art; `frontend/looks/<name>-<dark|light>.webp`, 224 px). Once a panel has shown a look while its settings were open, the page keeps that panel's own screen as the picture of that look instead (in the browser, newest first). So the picture of the look a panel uses is its real screen with its own content.

What a panel can do (its touch screen, pictures, web view, video, microphone, speaker, sensors) comes from the panel itself (`caps` in its device report); sections, page types, looks and options it cannot do are left out or shown greyed with the reason. "Sending to the panel…" turns into "Up to date on the panel" once the integration has stored the change and sent it on.

The page uses the websocket commands `panelos/page/subscribe`, `panelos/page/options`, `panelos/page/command`, `panelos/page/screen`, `panelos/page/resources`, `panelos/page/house` (alerts and status screens), `panelos/page/test` (show one on a panel for a few seconds), `panelos/page/alarms` (the panel's alarms), `panelos/page/ma` (music from Music Assistant for an alarm), `panelos/page/ma_player` (the panel's player in Music Assistant), `panelos/page/touch_test`, `panelos/page/proximity_calibration` and `panelos/page/forget_user` (administrators only; the last one forgets the panel's user, see below); the panel sends the picture as a JPEG with `panelos/screen` when asked (command `screenshot`).

## Alerts and status screens

Stored once for all panels (`.storage/panelos.house`), each item with the panels it appears on:

- **Critical alerts**: binary sensors that count as critical (suggested by their device class: smoke, carbon monoxide, gas, water leak, problem, tamper, safety), and own critical alerts with their own headline and line, started by a device (on or off, unlocked or locked, above or below a value) or by an automation (`panelos.critical_alert` / `panelos.clear_critical_alert`). While one is on, every panel it is shown on takes the whole screen in red with its sound (SOS by default; another one, or none), even in do not disturb, until it clears; holding the button silences it when the alert allows holding.
- **Status screens**: a state at home shown on the panels in its own colour (a room in use, a call going on, the robot vacuum at work …), with the panel's own words for its kind unless it has its own headline and line. Triggers: an entity's state (on, off, above or below a number, equal to a text), "after" another status screen ends (with a countdown), or an automation (`panelos.show_status` / `panelos.clear_status`). A screen that follows an entity stays while the entity's state holds: saving the page, changing a panel's options or reloading the integration does not end it, and neither does the entity being unavailable or unknown for up to two minutes (its own integration reloading, a device reconnecting); it ends when the entity comes back in a state that does not hold, or when it is still not back after two minutes. `panelos.clear_status` ends a screen started by an automation or by another screen's end (named by its ID or its name in automations); one that follows an entity ends with the entity.

A panel that connects gets what is active at that moment. The panels also list the active ones on their notifications page, above the notifications.

## Sounds

Timers, alarms and critical alerts each have a sound, picked on the PanelOS page with a button to listen: one of the panel's own sounds (Glass chimes, Marimba, Birds, Mechanical bell, Digital, Synth; for critical alerts also SOS and Warning tone), or one uploaded on the page (MP3, WAV, OGG or FLAC up to 10 MB). The defaults: a timers page rings with its look's own sound, alarms with Birds, critical alerts with SOS. An uploaded sound is converted once with Home Assistant's ffmpeg (16-bit WAV, mono, at most a minute; it loops while it rings) and kept in `/config/panelos/sounds`; every panel that uses it fetches it (`GET /api/panelos/sound/<id>`, with its login) and keeps a copy, so it rings even when Home Assistant is away.

The alarms live on the panel (they ring without Home Assistant); the timers page's settings on the PanelOS page show them and change them (`panelos/page/alarms`; the panel reports its list with `alarms` in `panelos/report`). An alarm can also ring with music from Music Assistant (its playlists and radio stations, or what a search finds; `panelos/page/ma`): the panel asks Music Assistant to play it on the panel's own media player, starting quietly and getting louder over half a minute; if no music comes within 12 seconds, the alarm's default sound rings instead. Music Assistant has the panel as a player by itself: the panel is a Sendspin player, paired through Home Assistant with no setup (see [docs/app.md](app.md#voice-satellite)). While an alarm has music, the timers page's settings show whether Music Assistant has the panel's player, with **Add to Music Assistant** (or the steps to add it by hand) when it has not.

## The panel's media player

The panel is a media player in Home Assistant (an entity of its voice device): Home Assistant, Music Assistant and automations can play on its speaker (`media_player.play_media`, volume, mute, pause, stop, announcements). Home Assistant's media proxy turns any source into a 48 kHz WAV stream, which the panel plays as it arrives. The volume set there is a share of the panel's own volume.

Music commands said to a panel (play, next, pause, volume, stop) can follow the panel: the panel reports the player picked on its music page (`music_player` in `panelos/report`), and `hass.data["panelos"]["voice_player"](satellite_entity_id)` returns the player for a panel's voice satellite: the picked player (else the integration's media player, else the panel's own Music Assistant player), or the panel's own player when Music commands is set to This panel; `None` for a satellite that is no panel's. A voice setup that keeps a player per satellite (for example a custom intent handler) calls it at the moment of the command, so the player always follows the music page.

## Music by voice

Home Assistant's own media commands work on a panel with no setup: said to the panel's voice satellite without naming a player, a kind of device, an area or a floor, they act on the player of its music page shown last (the first music page's until one has been shown; or on the panel itself, see Music commands above). Home Assistant brings the sentences, in every language it supports and in many variations, for example:

- **Play**: "play Queen", "play Bohemian Rhapsody" (searched on the player, the first result plays)
- **Next**: "next song", "skip this", "skip to the next track"
- **Previous**: "previous track", "go back to the previous song", "play the last song again"
- **Pause / stop / resume**: "pause", "pause the music", "stop the music", "resume", "unpause the music"
- **Volume**: "volume up", "turn the volume down", "set the volume to 30 percent", "mute", "unmute"

A command that names a player, a kind of device ("pause the TV"), another room or a floor ("pause the kitchen"), and every command said to another device, goes to Home Assistant's own handler unchanged; so does a search when the chosen player cannot search and play, and every command while the chosen player is unavailable. A chosen player that cannot do what was asked (a radio without next, a speaker without volume) gets Home Assistant's own reply for a device that does not support it. A conversation agent's tool calls carry the panel too, so what an AI agent does with a command follows the same rule (Home Assistant has no built-in sentence for a plain "play"; an agent turns it into resume). The reply is Home Assistant's own, in the language of the command (`custom_components/panelos/voice_intents.py`; the handlers of Home Assistant's media intents are wrapped once Home Assistant has started, and again when a reload or a newly set up integration registers them anew). Custom sentences of your own for the same commands take precedence over Home Assistant's built-in ones; to have them follow the panel too, ask `hass.data["panelos"]["voice_player"]` for the player.

## Dashboard mode

Dashboard mode is a secondary function. The panel is made first of all for the assistant: the voice satellite and the assistant mode's pages. A Home Assistant dashboard in the web view works within the limits of the panel's hardware (four Cortex-A35 cores, a Mali-G31 GPU, 2 GB of RAM), so a dashboard made for the panel should be light and simple: a few cards on a view, few custom cards, no heavy effects (blur behind bars, large animations), live camera streams only where they matter. A dashboard that is pleasant on a computer can take tens of seconds to load on the panel and scroll slowly.

The panel shows the dashboard as a browser does, custom cards included. It is logged in as its own Home Assistant user, so that user must be allowed to see the dashboard: a dashboard for administrators only, or views shown only to some users, stay empty on the panel.

While a dashboard loads, the panel shows its loading screen over Home Assistant's own start-up and reveals the dashboard once it has settled: cards drawn, icons in, nothing changing any more (at most 45 s).

Options for dashboard mode, besides the dashboard, header and sidebar:

- **Size** (`zoom`, 50–150 %, default 100): how large the dashboard is drawn; below 100 % more of it fits on the screen. It adds no extra step (WebKit lays the page out at that size), but below 100 % more cards are in view to lay out and draw, and icons and pictures are scaled. A dashboard designed for 480 × 480 at 100 % is lighter and sharper than a scaled one; use Size for a dashboard made for other screens.
- **Loading screen** (`loading_screen`, default on): off, Home Assistant's own start-up shows instead and the dashboard appears while it loads, sooner but unfinished at first.
- **Leave out unused card resources** (`skip_unused_resources`, default on): Home Assistant loads every card resource on every dashboard, used or not. The integration works out from the dashboard the panel shows which ones it needs (a resource counts as needed when its name matches a custom card type the dashboard uses or one of its top-level settings that is not switched off, for example `kiosk_mode`; an icon set counts when its file name has a word starting with "icon" and an icon prefix the dashboard uses, for example `hass-hue-icons.js` for `hue:` icons) and the panel leaves out the rest. It is worked out again when the panel shows another dashboard and when a dashboard is saved. In a test this cut a dashboard's first load from about 21 s to 15–17 s.
- **Always load these card resources** (`keep_resources`): for a resource a dashboard needs although its name matches none of its cards.

## Sensor reporting

Every value the panel sends is a row in the recorder's database (history, logbook, long-term statistics). The panel therefore sends a sensor only when it has changed by at least a set step from the value it sent last:

| Option | Default | Range | How it is measured |
|---|---|---|---|
| Illuminance | 15 % | 1–100 % | Checked twice a second; the step is a share of the last value sent and at least 5 lx, so the noise of a dark room is not sent. At most one change is sent in 10 seconds, and the value again every 10 minutes. |
| SoC temperature | 1.0 °C | 0.1–10 °C | Read every minute, averaged over about five minutes (the sensor works in steps of about 0.45 °C and flips between neighbouring steps). |
| Wi-Fi signal | 3 dB | 1–20 dB | Read every minute, averaged over about five minutes. |

Lower values give a finer history and automations that react to smaller changes, at the cost of more database writes. Higher values mean fewer writes, and only larger changes show up. In a test these three sensors sent about 3000 values a day without these steps; the same day's readings replayed with the defaults gave about 200. The panel's own automatic brightness, theme and Wi-Fi roaming use the full readings and do not depend on these options. Without the integration the panel uses the defaults.

## How it talks to the panel

The panel uses its own WebSocket connection to Home Assistant (its own user, no administrator rights needed):

- Only the panel's own user and administrators may use the commands below. The panel names that user: on every connection it asks Home Assistant who it is (`auth/current_user`) and reports the user's id as the Panel user sensor of its ESPHome device (counted only when the ESPHome entry has its key). The integration keeps that user in the panel's entry (the PanelOS page shows it under Device, About) and follows it when the panel is paired with another user. A panel that names no user (an older app, the sensor unavailable) keeps the first user that is not an administrator to subscribe for it; then the PanelOS page has Forget, and the next user that is not an administrator to connect is kept. Another user gets `unauthorized` on `panelos/subscribe` (and `not_found` for a panel without an entry), `not_subscribed` on the others. A subscription whose user is no longer the panel's (another one named or kept, forgotten, the entry deleted) ends with that error, and the panel asks again. Administrators always pass; one is kept only when the panel names it as its user.
- It subscribes with `{"type": "panelos/subscribe", "panel_id": "nspanel-pro"}` (`panel_id` is the panel's ID, see [Several panels](#several-panels); every message below carries it). Events carry `{"config": {...}}` (now and after every options change; `{}` when there is no entry for the panel, for an administrator; the pages, `screensaver`, `dnd`, `popups`, `theme_auto` (`by`: `light`, `sun` or `times`, with `from` and `to` for the dark hours), `wifi_roaming` (true by default), `wake_distance` (`near`, `medium` or `far`), the reporting steps as `illuminance_change`, `soc_temperature_change` and `wifi_signal_change`, and `time_zone`: Home Assistant's time zone as `name` and `posix`, its POSIX TZ rule from the footer of the zone's TZif file, because the panel has no zone database; for the few zones whose footer does not describe the present, such as Morocco's, the offset in effect as a fixed rule, looked at again every hour; the settings are sent again when Home Assistant's time zone changes), `{"show": {...}}` (an action call, a status screen `status` / `status_clear`, a critical alert `critical` / `critical_clear`, the panel's PanelOS notifications `notes`, its alarm list `alarms`, `touch_test` or `proximity_calibration`) or `{"command": {"key": "theme", "value": "Dark"}}` (an entity of the panel's device was set; the keys are the ones below).
- It reports with `{"type": "panelos/report", "panel_id": "nspanel-pro", "device": {...}, "options": {...}, "states": {...}}`: the device (name, maker, model, version, MAC, what it can do as `caps`) once per connection, the options of the selects and the entity states (`screen`, `dashboard`, `theme`, `brightness`, `auto_brightness`, `screensaver`, `screensaver_style`, `proximity_wake`, `child_lock`, `rain_on_glass`, `lightning`, `time_format`, `display_mode`, `illuminance`, `presence`, `soc_temperature`, `wifi_signal`, `ip_address`, `app_version`) when they change. Only the connection that holds the panel's subscription may report for it.
- A page's look, options or entities set on the panel itself (its menus, the calendar's last used view, the player picked on a music page) go back with `{"type": "panelos/page_set", "panel_id": "nspanel-pro", "index": 2, "variant": 1, "options": {"last_view": 3}}`: `index` counts the pages the panel shows, so each page of a type is set on its own (the panel names the page it was set on); an option set to `null` is removed; a value is plain, or a list of up to 64 items (`{"hidden_calendars": ["calendar.films"]}`); `entities` (a list of entity ids) replaces the page's entities. The integration checks the page as a whole, keeps it in the panel's options and sends the new config back. Only the subscribed connection may send it. A custom page's widgets (`items`, from the edit mode) are refused with `not_allowed` while "Allow editing on the panel" is off, also from an administrator; the look and the other options are taken either way.
- When its Home Assistant address is changed in its settings, the panel asks its old connection `{"type": "panelos/ha_addresses"}` and gets `{"addresses": [...]}`: the IPv4 and IPv6 addresses of Home Assistant's enabled network adapters (without loopback and link-local ones). With these and `internal_url` / `external_url` from `get_config` it tells whether the new address is the same Home Assistant (the login goes along) or not (the old login ends and the panel pairs again, see [security.md](security.md#home-assistant-connection)). Administrators and a connection that holds a panel's subscription may ask; another one gets `not_subscribed`.
- The config also carries `room`: the area of the panel's device in Home Assistant (empty when it has none), and `person_entities`: every `person.*` entity, in entity id order (sent again when a person is added or removed). A People widget keeps the people it shows as `people` (person ids; without it, everyone) and its switches in `opts` (1 the ones away too, 2 initials only; without bit 2 it shows the people's `entity_picture`, which the panel fetches from Home Assistant itself); while there is a People widget the panel follows the states of every person (its menu offers them all). The older top-level `people` (a list of person ids) is still the default of a People widget without its own list, and `show_people: true` still puts small heads at the bottom of every page (off unless set).
- Timers are Home Assistant's own timers of the panel's voice device (the ones voice starts and stops), so voice, the panel and other devices see the same timers. The panel starts and changes them with `{"type": "panelos/timer", "panel_id": "nspanel-pro", "action": "start", "seconds": 300, "name": "Pasta"}` (actions `start`, `pause`, `resume`, `add`, `remove`, `cancel`; all but `start` take the `timer_id`; a `name` longer than 30 characters is cut to 30, as everywhere a timer gets its name); the timer events reach the panel over its ESPHome connection as for any voice satellite, and right after `panelos/subscribe` the panel gets `{"timers": [...]}` with the ones running. The panel's settings also name the speech recognition of its assistant (`stt`: engine and language, from the satellite's pipeline): what is said while a timer rings is checked with it for a stop word. Its text-to-speech (`tts`: engine, language and voice, from the same pipeline; else Home Assistant's default engine) says the replies at the door: the panel calls `tts.speak` with it on the cameras page's speaker at the door; a cameras page without a chosen speaker gets the media player on a camera's own device. Without Home Assistant (or when it refuses a timer) the panel runs the timer itself, and hands it over to Home Assistant (with the time left) once the integration answers again.
- Voice assistants on any device can put a timer on a panel: the integration gives every agent that uses the Assist API three tools (`custom_components/panelos/llm.py`, Home Assistant's `llm` platform): `panelos__StartPanelTimer` (the panel by its room or name, hours / minutes / seconds, a name of up to 30 characters; a longer one is cut), `panelos__CancelPanelTimer` and `panelos__ListPanelTimers`, with a line in the prompt naming the panels' rooms and saying that the time left can be told. "Set a 10 minute timer on the kitchen panel", said to a speaker in another room, becomes a timer of the kitchen panel's voice device: the panel shows it and rings it as its own, and it stays one of Home Assistant's timers. Home Assistant's own timer tools always use the device that is spoken to. No other setup is needed.
- Alarms by voice: three more tools set, list and cancel the alarms a panel keeps and rings itself (the same alarms as on its timers page and the PanelOS page): `panelos__SetPanelAlarm` (the panel by its room or name, hour and minute, repeat days as weekday names or "weekdays" / "weekends" / "every day", once without days, a name; an alarm that already exists at that time and on those days is switched on instead of added again), `panelos__ListPanelAlarms` (one panel or all) and `panelos__CancelPanelAlarm` (by time and / or name, or all; deleted, or only switched off with keep; a snoozed alarm's snooze ends too, also for a once alarm that is off already because it rang). A change goes to the panel as its alarm list, kept until the panel confirms it (as from the PanelOS page); a panel that has never reported its alarms is not changed. "Wake me at 6:45 on weekdays with the bedroom panel" works from any device; these tools are offered for every panel, also one without a voice device.

## Example: doorbell

```yaml
actions:
  - action: panelos.show_view
    data:
      view: camera
      entity_id: camera.front_door
      seconds: 45
```

## Licence

AGPL-3.0-or-later, like the rest of the project.
