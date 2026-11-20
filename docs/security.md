# Security

What PanelOS protects, how, and what is left open on purpose while the project is in development.

## Network services

The panel listens on:

| Port | Service | Who may use it |
|---|---|---|
| TCP 6053 | ESPHome native API (`app/src/esphome.rs`, `app/src/api_link.rs`) | Home Assistant only, encrypted |
| TCP 8928 | Sendspin player (`app/src/sendspin.rs`) | a paired Music Assistant; end-to-end encrypted (Noise KKpsk2), no unpaired access |
| TCP 22 | SSH (dropbear) | the holder of the SSH key in the image |
| UDP 5353 | mDNS (`app/src/mdns.rs`) | answers only to the panel's own network |
| TCP 8080 | login callback (`app/src/pairing.rs`) | only until the panel is paired |

There is no firewall (the kernel is built without netfilter); every service above checks its own clients.

Sendspin: from 0.0.7 a host that has not paired with the panel can only pair. Up to 0.0.6 an unpaired host on the network could also start, stop, clear and feed the music.

## Outgoing connections

The panel connects to:

- **Home Assistant** (its URL in `app.json`): the login, the WebSocket session, the REST calls (camera pictures, calendars, to-do lists, sounds, speech recognition) and the web view's dashboard. Plain `http://` and `ws://` (see below).
- **NTP:** `pool.ntp.org` (busybox `ntpd`, `S45ntp`), once Wi-Fi has an address. Until NTP has set the clock, the panel takes the time from Home Assistant.
- **Hosts named by Home Assistant**, without the login token:
  - artwork whose picture URL is on another host (a player's `entity_picture`), no redirects, up to 8 MB;
  - a notification's picture when it is a full URL, up to 8 MB;
  - a URL Home Assistant plays on the panel's media player (see the ESPHome API).
  Pictures are decoded by memory-safe Rust decoders (JPEG, PNG), at most 4096 × 4096 pixels.
- **The web view** loads the dashboard from Home Assistant and whatever that page refers to (card resources, pictures), like any browser.

The app itself speaks no TLS (its HTTP client is built without it), so it can only reach `http://` URLs.

## ESPHome API

- **Only Home Assistant connects.** The panel takes connections only from the address of its Home Assistant URL (`ha_url` in `app.json`). If Home Assistant reaches the panel from another address (a proxy, a container network), add it to `app.json`: `"api_allow": ["192.168.1.10"]`.
- **Encrypted.** Like an ESPHome device without a key in its YAML, the panel reports that it supports encryption and can be given a key. Right after adding the panel, Home Assistant creates a key, sends it over a Noise session with the all-zero key, and connects encrypted from then on (`Noise_NNpsk0_25519_ChaChaPoly_SHA256`, the `snow` crate). The key is in `/mnt/persist/nspanel/api_key` (0600). From then on the panel refuses plaintext and every other key.
- **New key or a new Home Assistant:** delete the panel in Home Assistant (which clears the key there), then on the panel `rm /mnt/persist/nspanel/api_key` and restart the app; add the panel again.
- **Limits:** plaintext frames up to 256 KB, Noise frames up to 64 KB, at most three connections, an idle connection that is not the main session closes after 60 s. A malformed message ends only its own connection, never the API.
- **Voice:** a run streams the microphones for at most 30 s whatever the events say; Home Assistant's "continue the conversation" only counts while the panel's own run is going on.
- **Audio from URLs** (answers, announcements) is fetched only from Home Assistant's host, without redirects, up to 16 MB, at 8–48 kHz, 1–2 channels and 3 minutes; one announcement at a time.
- **The media player** (Home Assistant, and Music Assistant through it, plays a URL on the panel; alarms with music) streams the URL Home Assistant sends, normally its media proxy. It follows up to two redirects and does not check the host (`app/src/media.rs`). This is on purpose and not a gap: only Home Assistant can send the command, over the encrypted API, and the URL is normally Home Assistant's own proxy that turns the source into the WAV stream the panel announces; no login token goes with the request, and the panel only plays what it gets as sound. Whoever can make Home Assistant play a URL on the panel controls Home Assistant anyway.

## Speech while a timer or an alarm rings

While a timer or an alarm rings (and the microphones are not muted), the panel listens over the ring for a stop phrase, outside any voice run (`app/src/voice.rs`, `app/src/stt.rs`). A piece of speech (at most 2.5 s, with half a second before it) and every "Stop" the wake word engine hears go to Home Assistant's speech-to-text (`POST /api/stt/<engine>`, the engine and language of the panel's own assistant), at most three at a time. The text only decides whether the ring stops: it never goes to an assistant. It is written to the system log ("heard over the ring").

## Home Assistant connection

- The panel logs in with Home Assistant's normal login (OAuth, QR code). The refresh token is in `/mnt/persist/nspanel/ha_auth.json` (0600) and never logged. Logging out revokes it in Home Assistant.
- The app's own requests send the login token only to Home Assistant itself (camera, artwork, sounds, speech recognition), never to another host. The web view gets it too (below).
- **Pictures of the screen** for the PanelOS page in Home Assistant are taken only when an administrator has the page open, and go only to Home Assistant over the panel's own connection.
- **No TLS yet:** the connection is plain `http://` and `ws://`, readable by anyone on the same network. Use the panel only in a network you trust, and pair it with a Home Assistant user that is not an administrator.

## Web view

The dashboard mode shows Home Assistant's frontend in WPE WebKit (`panelos-web`, `buildroot/package/panelos-web`). The browser comes in its own SquashFS image (`/mnt/persist/panelos-web.squashfs`, mounted read-only at `/opt/panelos-web` by `S50panelos-web`) and the app starts it chrooted there, with `/dev`, `/proc` and `/sys` bound in and its data in `/mnt/persist/panelos-web-data` (0700). It runs as root: the chroot keeps its files apart, not its rights.

It logs in like Home Assistant's mobile apps (external authentication): the page asks for a token, and the app answers with the panel's access token. The web view does not check which page asks and does not limit where the page navigates.

## Home Assistant integration

- **The PanelOS page** in the sidebar is for administrators only, and so are the WebSocket commands behind it (settings, pictures of the screen, the touch test, the proximity calibration, the alerts and statuses).
- **Its actions** (`panelos.show_view`, `notify`, `show_status`, `clear_status`, `critical_alert`, `clear_critical_alert`) are admin-only (`async_register_admin_service`).
- **Uploaded sounds:** only administrators upload (`POST /api/panelos/sounds`). Any logged-in user can download one by its id (`/api/panelos/sound/<id>`), which is how the panels keep their copies.
- **The panel's own commands** (`panelos/subscribe`, `report`, `page_set`, `timer`, `notification`, `alive`, `log`, `screen`) are open to every logged-in Home Assistant user, since the panel uses an ordinary user (see Not covered).
- **Voice assistants** that use the Assist API get tools to start, list and cancel timers and to set, list and cancel alarms on a panel named by its room (`llm.py`; at most 20 alarms a panel), from any device.

## At the panel

Anyone who can touch the panel uses its Home Assistant user's rights through it:

- the quick controls switch their entities;
- calendar events can be edited and deleted on calendars that allow it (a delete can be taken back for 5 s), and to-do items added and ticked;
- while a door camera rings, up to four replies set in the integration can be said on the speaker at the door (`tts.speak`);
- with "Allow editing on the panel" on in the integration (off by default, per panel), a long press and holding the lock edit the custom pages: widgets, layout, background.

The **child lock** (a switch in Home Assistant) keeps touches from the pages until the lock is held for 3 s, and locks again after a minute without a touch. It keeps small hands out; it is not access control.

## Logs kept on the panel

- The system log is in RAM (`/var/log/messages`); `S98logforward` also copies it into pstore (see the development aids).
- **Connection log**, also on a release install: Home Assistant sessions ending and coming back with the cause, the Wi-Fi roam checks with the access points they saw, moves and reconnects, in `/mnt/persist/nspanel/connection.log` (the last ~256 KB in two files, `app/src/connlog.rs`).
- **Development install** (the UI from `/mnt/data/panelos-ui-dev`): the whole system log, voice lines included, is copied every second to `/mnt/data/panelos-log/panel.log` (2 MB, three old files kept) and sent to Home Assistant, which keeps it in `<config>/panelos/logs/<panel>.log` (5 MB, three old files kept) (`app/src/devlog.rs`). A release install keeps none of this.

## SSH

- Root logs in with the SSH key only; the root password is locked, also on the serial console.
- The host key is created on the first start and kept in `/mnt/persist/nspanel/dropbear_ed25519_host_key` (`S49sshhostkey`), so it survives reboots and updates. The commands in the guides accept it the first time and check it after (`/root/.ssh/panelos_known_hosts`). If it ever changes on purpose (e.g. after restoring the stock firmware): `ssh-keygen -R <panel IP> -f /root/.ssh/panelos_known_hosts`.

## Build

- Kernel and Buildroot are fetched at fixed commits (`tools/fetch_sources.sh` stops when a tag moved).
- Wake word models come from a fixed commit or a release and are checked against `tools/wakewords.sha256`.
- Rust crates are pinned with checksums in `app/Cargo.lock`.

## Development aids

These stay on while PanelOS is in development; they make testing and debugging possible without opening the panel. A release image has to leave them out.

| Aid | What it does | Risk |
|---|---|---|
| SSH key in the image (`NSPANEL_SSH_PUBKEY_FILE`) | root login for the builder | a release image must not carry anyone's key |
| `S98logforward` | copies syslog lines into the kernel log, so they survive in pstore and can be read after a crash | log lines (below) outlive a reboot |
| Log lines of the voice satellite | every pipeline event with the first 100 characters of its values (heard text, answer), what is heard over a ring, timer names, show requests | private speech in `/var/log/messages` (RAM) and pstore |
| `/mnt/data/panelos-ui-dev` | the UI from this directory instead of the bundle image (`S51panelos-ui`), and the development log copies (above) | whoever writes there replaces the app; the whole log on flash and in Home Assistant |
| `NSPANEL_DEV_INPUT` | a FIFO (0600) that takes touches, gestures and app actions as if tapped, page changes and test states | root only |
| `SIGUSR1` | screenshot to `/tmp/nspanel-screen.ppm` | may capture a message or a camera |
| `SIGUSR2` | scrolls the dashboard one page | root only |
| `/tmp/nspanel-voice-test.wav`, `/tmp/nspanel-voice-followup.wav` | sent to Home Assistant as speech instead of the microphones | root only |
| `/tmp/nspanel-ring-dump` | every piece of speech heard over a ring also saved as a WAV (the last 20) | private speech in RAM |
| `/tmp/nspanel-aec-cold`, `/tmp/nspanel-aec-warm` | echo canceller modes for measurements | root only |
| `/tmp/nspanel-gpu-off` | the image's software-rendered app instead of the GPU app (`S90nspanel-app`) | root only |
| `/tmp/nspanel-proximity-test` | proximity test view (`docs/bringup.md`) | root only |
| `/tmp/nspanel-touch-test` | touch test view (`docs/bringup.md`) | root only |
| `NSPANEL_API_DEBUG`, `NSPANEL_AEC_DUMP` | API message log, microphone dump | root only |
| `NSPANEL_FRAME_STATS`, `NSPANEL_NO_WEB`, `NSPANEL_NO_VOICE`, `PANELOS_GLASS` | frame times in the log; no web view; no microphones, wake words or player; the rain-on-glass GPU pass off | root only |
| `NSPANEL_WEB_STATS`, `NSPANEL_DEV_DARK`, `WEBKIT_*` | web view frame timing in the log; backlight kept off; WebKit's own switches passed to the web helper | root only |
| `/mnt/persist/panelos-web-data/console-log` | the dashboard web view writes the page's console messages to the log | dashboard details in `/var/log/messages` (RAM) |
| debugfs, `/dev/mem`, magic SysRq, unsigned kernel modules | kernel debugging | root only |

All of these need root on the panel, which needs the SSH key.

## Not covered

- **No verified boot.** The stock U-Boot starts whatever is in the boot partition; physical access (BOOT button and USB) or root on the panel means full control. The UI and web bundles are not signed either.
- The panel app runs as root, and so does the web view.
- **Any Home Assistant user can act as a panel** towards the integration: the panel's commands (above) do not check that the user is the panel's. Another user of the same Home Assistant can read a panel's settings, change its pages and, on a development install, add lines to its log file. Give Home Assistant accounts only to people you would let use the panel.
- **The web view hands the token to the page it shows** without checking where that page comes from. A dashboard that opens another site in the web view gives that site the means to ask for the panel's token.
