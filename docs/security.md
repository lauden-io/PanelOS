# Security

What PanelOS protects, how, and what is left open on purpose while the project is in development.

A panel like this mostly sits on a home network, behind the router, among people who live there. I still aimed for the most security I can give it, and treat everything that reaches it over the network as untrusted:

- the Home Assistant login token goes only to the address it was made for, never through a redirect, and a new address keeps the login only when that Home Assistant itself names it as one of its own;
- every service checks its own clients (there is no firewall to lean on): the ESPHome API by address and key, Sendspin by its Noise handshake, with limits on connections, message sizes and handshake time;
- in the Home Assistant integration, a user who has not been shown to be the panel's own cannot change which device counts as the panel, and widgets sent from the panel are refused while editing on the panel is switched off;
- input from the network is bounded before it is held in memory (pictures, sounds);
- the build takes its dependencies only as locked, and the prebuilt Skia library only when its SHA-256 matches the pinned one.

What is still open, and why, is listed below with the rest.

## Network services

The panel listens on:

| Port | Service | Who may use it |
|---|---|---|
| TCP 6053 | ESPHome native API (`app/src/esphome.rs`, `app/src/api_link.rs`) | Home Assistant only, encrypted |
| TCP 8928 | Sendspin player (`app/src/sendspin.rs`) | a paired Music Assistant; end-to-end encrypted (Noise KKpsk2), no unpaired access |
| TCP 22 | SSH (dropbear) | the holder of the SSH key in the image |
| UDP 5353 | mDNS (`app/src/mdns.rs`) | answers only to the panel's own network |
| TCP 8080 | login callback (`app/src/pairing.rs`) | only until the panel is paired |

There is no firewall (the kernel is built without netfilter); every service above checks its own clients. The ESPHome API checks the address and the key, SSH the key. The login callback (8080) and Sendspin (8928) take a connection from any host: the callback acts only on a request that carries the login's random `state` and a one-time code Home Assistant accepts, and Sendspin decides by its Noise handshake.

Sendspin: any host can connect and finish the Noise handshake (a host that has not paired, with the published sentinel key). After it, a host that has not paired can only pair, and pairing needs the pairing key, which Home Assistant asks for over the encrypted ESPHome API. Sendspin takes at most 8 connections, at most 4 of them from hosts that have not shown a paired key (a fifth closes the oldest of those, so a host that holds connections open cannot lock the music server out; a host that keeps reconnecting fast can still disturb Music Assistant's new connections, a denial of service from inside the network), closes a connection whose handshake is not done 30 s after its start, and refuses messages over 96 KiB. A pairing record is written only for a session that used the pairing key, so only a holder of that key can push one of the 8 records out. Sendspin came in 0.0.6; there a host that had not paired could not start the music, but it could stop and clear it, feed it sound while it played and set the music's clock.

## Outgoing connections

The panel connects to:

- **Home Assistant** (its URL in `app.json`): the login, the WebSocket session, the REST calls (camera pictures, calendars, to-do lists, the media player list, profile pictures, sounds, speech recognition) and the web view's dashboard. Plain `http://` and `ws://` (see below).
- **NTP:** `pool.ntp.org` (busybox `ntpd`, `S45ntp`; an image built before 0.0.7 also asks a regional pool), once Wi-Fi has an address. Until NTP has set the clock, the panel takes the time from Home Assistant.
- **Hosts named by Home Assistant**, without the login token:
  - artwork whose picture URL is on another host (a player's `entity_picture`), no redirects, up to 8 MB;
  - a person's profile picture (People widget) when its address is on another host, no redirects, up to 8 MB;
  - a notification's picture when it is a full URL, up to 8 MB;
  - a URL Home Assistant plays on the panel's media player (see the ESPHome API).
  Pictures are decoded by memory-safe Rust decoders: camera pictures, artwork and notification pictures as JPEG, profile pictures as JPEG or PNG. The limit counts all pixels (width times height), not each side: at most 4096 × 4096 in all, a profile picture at most 2048 × 2048 in all. A PNG profile picture is checked before decoding; a JPEG one is first decoded under the 4096 × 4096 limit and checked after.
- **The web view** loads the dashboard from Home Assistant and whatever that page refers to (card resources, pictures), like any browser.

The app itself speaks no TLS (its HTTP client is built without it), so it can only reach `http://` URLs.

## ESPHome API

- **Only Home Assistant connects.** The panel takes connections only from the address of its Home Assistant URL (`ha_url` in `app.json`; a host name there is looked up at each connection). If Home Assistant reaches the panel from another address (a proxy, a container network), add it to `app.json`: `"api_allow": ["192.168.1.10"]`. `api_allow` takes IP addresses only; a host name in it is ignored.
- **Encrypted.** Like an ESPHome device without a key in its YAML, the panel reports that it supports encryption and can be given a key. Until it has one, it takes plaintext as well as Noise with the all-zero key from those addresses, and a key sent either way is stored: whoever connects from such an address first can set it. Right after adding the panel, Home Assistant creates a key, sends it and connects encrypted from then on (`Noise_NNpsk0_25519_ChaChaPoly_SHA256`, the `snow` crate). The key is in `/mnt/persist/nspanel/api_key` (0600). From then on the panel refuses plaintext and every other key. A session with the key can also clear it (an empty key), which brings back the state without a key.
- **New key or a new Home Assistant:** delete the panel in Home Assistant (which clears the key there), then on the panel `rm /mnt/persist/nspanel/api_key`; add the panel again. The panel reads the key file at every connection, so the app needs no restart.
- **Limits:** plaintext frames up to 256 KB, Noise frames up to 64 KB, at most three connections, an idle connection that is not the main session closes after 60 s. A malformed message ends only its own connection, never the API.
- **Voice:** a run streams the microphones for at most 30 s whatever the events say; Home Assistant's "continue the conversation" only counts while the panel's own run is going on.
- **Audio from URLs** (answers, announcements) is fetched only from Home Assistant's host, without redirects, up to 16 MB, at 8–48 kHz, 1–2 channels and 3 minutes; one announcement at a time.
- **The media player** (Home Assistant, and Music Assistant through it, plays a URL on the panel; alarms with music) streams the URL Home Assistant sends, normally its media proxy. It follows up to two redirects and does not check the host (`app/src/media.rs`). This is on purpose and not a gap: only Home Assistant can send the command, over the encrypted API, and the URL is normally Home Assistant's own proxy that turns the source into the WAV stream the panel announces; no login token goes with the request, and the panel only plays what it gets as sound. Whoever can make Home Assistant play a URL on the panel controls Home Assistant anyway.

## Speech while a timer or an alarm rings

While a timer or an alarm rings (and the microphones are not muted), the panel listens over the ring for a stop phrase, outside any voice run (`app/src/voice.rs`, `app/src/stt.rs`). A piece of speech (at most 2.5 s, with half a second before it) and every "Stop" the wake word engine hears go to Home Assistant's speech-to-text (`POST /api/stt/<engine>`, the engine and language of the panel's own assistant), at most three at a time. The text only decides whether the ring stops: it never goes to an assistant. It is written to the system log ("heard over the ring").

## Home Assistant connection

- The panel logs in with Home Assistant's normal login (OAuth, QR code). The refresh token is in `/mnt/persist/nspanel/ha_auth.json` (0600) together with the Home Assistant address it was made for, and never logged. It is only ever sent to that address: a login file made for another address (`app.json` changed by hand) is revoked at its own address and deleted, and the panel pairs again. Logging out revokes it in Home Assistant.
- **A new address** set on the panel (settings, Home Assistant): the panel first asks the Home Assistant it is still connected to for that Home Assistant's own addresses (`internal_url` and `external_url` from `get_config`, and the IP addresses of its enabled network adapters from the integration, `panelos/ha_addresses`). If the new address is one of them, the login stays and moves to the new address: a host name only when scheme, name and port are exactly those of one of the two URLs or of the old address (a name is never looked up for this, since its DNS record could point elsewhere later); an IP address written out when it is in one of those URLs, or is one of the adapters' addresses on a scheme and port one of the URLs has. An address that URL parsers could read differently (with `\`, `@`, a user name or white space) never counts, and the setup does not try it. Otherwise, or when the old Home Assistant cannot be reached at that moment, the old login ends: it is revoked at the old address (best effort), the file is deleted, and the panel has to be paired again. Home Assistant's mDNS `uuid` is not used for this check: anyone on the network can read it, and a fake Home Assistant could copy it.
- The app's own requests send the login token only to the address the login was made for (camera, artwork, profile pictures, the media player list, sounds, speech recognition), never to another host. The WebSocket connection follows no redirects; the HTTP requests do not pass the token on when they are redirected (and send no body again). The web view gets it too (below).
- **Pictures of the screen** for the PanelOS page in Home Assistant are taken only when an administrator has the page open, and go only to Home Assistant over the panel's own connection.
- **No TLS yet:** the connection is plain `http://` and `ws://`, readable by anyone on the same network. Use the panel only in a network you trust, and pair it with a Home Assistant user that is not an administrator.

## Web view

The dashboard mode shows Home Assistant's frontend in WPE WebKit (`panelos-web`, `buildroot/package/panelos-web`). The browser comes in its own SquashFS image (`/mnt/persist/panelos-web.squashfs`, mounted read-only at `/opt/panelos-web` by `S50panelos-web`) and the app starts it chrooted there, with `/dev`, `/proc` and `/sys` bound in and its data in `/mnt/persist/panelos-web-data` (0700). It runs as root, and a root process with `/proc` and `/dev` in reach is not held by a chroot: the chroot keeps the browser's files apart, it is no boundary. WebKit's own sandbox (bubblewrap) is not built into the bundle (`BR2_PACKAGE_WPEWEBKIT_SANDBOX` is off), and the kernel has none of the namespaces it would need (`CONFIG_NAMESPACES` is off), so the web content runs without a sandbox, as root.

It logs in like Home Assistant's mobile apps (external authentication): the page asks for a token, and the app answers with the panel's access token. The token goes only to a page on the origin (scheme, host and port) of the dashboard address the app loads (Home Assistant's address and the dashboard's path): a request counts only while the view shows that origin, the answer checks the origin again in the page that receives it, and it never goes into a frame inside the page.

The view stays on that origin. A link, a script or a redirect that would put another site in the view is refused before that page replaces the dashboard, and new windows never open. Frames inside the dashboard (a webpage card) and what it loads from other hosts (pictures, card scripts) are not limited. Card resources run as part of Home Assistant's own page, so install only ones you trust.

## Home Assistant integration

- **The PanelOS page** in the sidebar is for administrators only, and so are the WebSocket commands behind it (settings, pictures of the screen, the touch test, the proximity calibration, the alerts and statuses, the alarms and their music, forgetting a panel's user).
- **Its actions** (`panelos.show_view`, `notify`, `show_status`, `clear_status`, `critical_alert`, `clear_critical_alert`) are admin-only (`async_register_admin_service`).
- **Uploaded sounds:** only administrators upload (`POST /api/panelos/sounds`). Any logged-in user can download one by its id (`/api/panelos/sound/<id>`), which is how the panels keep their copies.
- **Media players:** any logged-in user can read the list of available media players (`GET /api/panelos/players`), with each player's state and the title and artist it plays; the panels' music pages list them with it.
- **The page's files** (`/panelos_static`: the integration's `frontend` folder with the page script, icons, fonts, pictures and the built-in sounds) are served without a login.
- **The panel's own commands** (`panelos/subscribe`, `report`, `page_set`, `timer`, `notification`, `alive`, `log`, `screen`, `ha_addresses`) accept only the panel's own Home Assistant user and administrators. Another user is refused. For a panel that is not set up here, a subscription is refused too, unless it comes from an administrator.
- **Log files:** `panelos/log` writes the lines it gets to `<config>/panelos/logs/` whatever the panel's install (the panel sends them only from a development install, see "Logs kept on the panel"). The panel's own user writes only its panel's file; an administrator can name any file there.
- **The panel's entities** (the restart and reboot buttons, the screen, child lock and other switches, the selects, the brightness, the notify entity) follow Home Assistant's usual rights: any Home Assistant user can use them, as other entities, and so switch the child lock off.
- **The panel names its user.** On every connection it asks Home Assistant who it is logged in as (`auth/current_user`) and reports that user's id as the Panel user sensor of its ESPHome device. The sensor's state comes over the encrypted ESPHome connection, whose key only Home Assistant and the panel hold, and over Home Assistant's API only administrators can set an entity's state, so another user cannot pose as a panel that names its user. The integration counts the sensor only when the panel's ESPHome entry has a key. The panel's ESPHome entry is the one of the MAC address kept for the panel, as long as that entry is there, whatever other entries with the panel's ID say (Home Assistant takes a device's new name from the device itself). Without it (nothing kept yet, or the old panel's entry deleted after the panel was replaced), of the ESPHome entries with the panel's ID as their device name one whose device names a user comes first; while none names a user, another device's entry is never taken. A reported MAC address is kept only from an administrator, or when the ESPHome entry with that address has a key, names the reporting user, and no entry of the MAC address kept before is there. So which entry is the panel's follows from Home Assistant's own data, and connecting first gives no lasting binding: once the panel names its user, the binding moves to that user. Until a MAC address is kept for a panel (its first report once it names its user), a device of another user that Home Assistant already has with a key and that is renamed to the panel's ID could be taken for it. A panel whose ESPHome entry has no key never has its MAC address confirmed (only an administrator's report keeps one), so for it this stays possible: give the panel's ESPHome connection a key. The integration keeps that user in the panel's entry (the PanelOS page shows it under Device, About), and when the panel is paired with another user the binding moves by itself and the old user's subscriptions end.
- **A panel that names no user** (an older app, or the sensor unavailable) keeps the first user that is not an administrator to subscribe for it; so does a new entry, or one whose user was forgotten, until the panel names its user. Once a panel has named its user, that user stays kept through such a gap. The PanelOS page can forget the kept user of a panel that names none; the next user that is not an administrator to connect is kept then. Removing the panel or deleting the user forgets it too. Without the sensor, administrators are never kept, so with a panel that logs in as an administrator the first other user to connect would be. Give each panel its own user that is not an administrator anyway (see "No TLS yet").
- **Voice assistants** that use the Assist API get tools to start, list and cancel timers and to set, list and cancel alarms on a panel named by its room or name (`llm.py`; at most 20 alarms a panel), from any device and for any user: the tools check neither who asks nor what is exposed to the assistant.

## At the panel

Anyone who can touch the panel uses its Home Assistant user's rights through it:

- the quick controls switch their entities;
- the music pages control media players, Music Assistant included (play, pause, skip, seek, volume, moving the queue, grouping players);
- the thermostat pages set temperatures and modes;
- timers and alarms are set, changed and stopped (an alarm can start Music Assistant music when it rings);
- notifications can be dismissed, Home Assistant's persistent notifications for everyone;
- calendar events can be edited and deleted on calendars that allow it (a delete can be taken back for 5 s), and to-do items added and ticked;
- while a door camera rings, up to four replies can be said on the speaker at the door (`tts.speak`): the ones set in the integration, or four built-in ones when none are set; and "Hold to open", held for 1.5 s, opens the lock or cover set for the camera's page (`lock.unlock` or `cover.open_cover`);
- the settings (no PIN) change the Wi-Fi network and the Home Assistant address (a new address ends the login unless it is one of the same Home Assistant's own, see "Home Assistant connection") and restart the panel;
- with "Allow editing on the panel" on in the integration (off by default, per panel), a long press and holding the lock edit the custom pages: widgets, layout, background. With it off, the integration refuses a page's widgets sent with `panelos/page_set` (`not_allowed`), from the panel's user and from administrators alike; a page's look, its background settings and its menu options set on the panel are taken either way.

The **child lock** (a switch in Home Assistant) keeps touches from the pages until the lock is held for 3 s, and locks again after a minute without a touch or when the screensaver comes on. It is not over the ringing screen of a timer or an alarm (a tap stops it), a critical alert (it is held to silence it), or the setup and pairing screens. It keeps small hands out; it is not access control.

## Logs kept on the panel

- The system log is in RAM (`/var/log/messages`); `S98logforward` also copies it into the kernel log, from where it goes to the serial console (`loglevel=7`) and into pstore (see the development aids).
- **Connection log**, also on a release install: Home Assistant sessions ending and coming back with the cause, the Wi-Fi roam checks with the access points they saw, moves and reconnects, in `/mnt/persist/nspanel/connection.log` (the last ~256 KB in two files, `app/src/connlog.rs`).
- **Development install** (the UI from `/mnt/data/panelos-ui-dev`): the whole system log, voice lines included, is copied every second to `/mnt/data/panelos-log/panel.log` (2 MB, three old files kept) and sent to Home Assistant, which keeps it in `<config>/panelos/logs/<panel>.log` (5 MB, three old files kept) (`app/src/devlog.rs`). A release install writes none of this, but it removes nothing either: the files from a development install stay in `/mnt/data/panelos-log` and in Home Assistant's config folder (so in its backups too) until they are deleted.

## Secrets on the panel

They are kept in `/mnt/persist/nspanel` (0700, root only); the files the app writes there are 0600.

| File | What it holds |
|---|---|
| `ha_auth.json` | the Home Assistant refresh token, with the address it was made for |
| `api_key` | the ESPHome API key |
| `sendspin.json` | the Sendspin identity (its private key), the pairing key and the pairing records (each paired server's long-term key) |
| `wpa_supplicant.conf` | the Wi-Fi networks: for WPA2 the key made from the passphrase (`psk=`, enough to join that network), for WPA3 the password itself in plain text (`sae_password=`) |
| `dropbear_ed25519_host_key` | the SSH host key |

wpa_supplicant runs with a copy, `/tmp/wpa_supplicant.conf` (0600). While a network is added in the settings, its WPA2 key or WPA3 password is passed on `wpa_cli`'s command line, where every process on the panel can read it; a WPA2 passphrase itself goes to `wpa_passphrase` on its standard input and is not kept.

## SSH

- Root logs in with the SSH key only; the root password is locked, also on the serial console.
- The host key is created on the first start and kept in `/mnt/persist/nspanel/dropbear_ed25519_host_key` (`S49sshhostkey`), so it survives reboots and updates. The commands in the guides accept it the first time and check it after (`/root/.ssh/panelos_known_hosts`). If it ever changes on purpose (e.g. after restoring the stock firmware): `ssh-keygen -R <panel IP> -f /root/.ssh/panelos_known_hosts`.

## Build

- Kernel and Buildroot are fetched at fixed commits (`tools/fetch_sources.sh` stops when a tag moved).
- Wake word models come from a fixed commit or a release and are checked against `tools/wakewords.sha256`.
- Rust crates are pinned with checksums in `app/Cargo.lock`, and the app is built with exactly those (`cargo build --locked`).
- The GPU app's prebuilt Skia, which skia-bindings would download from GitHub without a checksum, is fetched by `tools/build_app.sh`, checked against `tools/skia-binaries.sha256` and only then handed to skia-bindings. The script works out beforehand which binaries skia-bindings will want and stops before the build when they are not pinned, so skia-bindings never falls back to building Skia from sources it downloads unchecked.

## Development aids

These stay on while PanelOS is in development; they make testing and debugging possible without opening the panel. A release image has to leave them out.

| Aid | What it does | Risk |
|---|---|---|
| SSH key in the image (`NSPANEL_SSH_PUBKEY_FILE`) | root login for the builder | a release image must not carry anyone's key |
| `S98logforward` | copies syslog lines into the kernel log, so they survive in pstore and can be read after a crash | log lines (below) outlive a reboot |
| Detailed log lines | every pipeline event of the voice satellite with the first 100 characters of its values (heard text, answer), what is heard over a ring, timer names, show requests; the Home Assistant user's name and id, the title and artist a music page shows, an alarm's music, the whole address of a failed download (a token in it included), IP addresses and access points (BSSIDs) | private speech and private details in `/var/log/messages` (RAM), on the serial console and in pstore |
| `/mnt/data/panelos-ui-dev` | the UI from this directory instead of the bundle image (`S51panelos-ui`), and the development log copies (above) | whoever writes there replaces the app; the whole log on flash and in Home Assistant |
| `NSPANEL_DEV_INPUT` | a FIFO (0600) that takes touches, gestures and app actions as if tapped, page changes and test states; its `web_load` puts any URL in the web view, whose origin then gets the login token | root only |
| `NSPANEL_AUTH`, `NSPANEL_CONFIG`, `NSPANEL_SETTINGS` | other files for `ha_auth.json`, `app.json` and `settings.json` (a second instance with a test Home Assistant) | root only |
| `SIGUSR1` | screenshot to `/tmp/nspanel-screen.ppm` | may capture a message or a camera |
| `/tmp/nspanel-voice-test.wav`, `/tmp/nspanel-voice-followup.wav` | sent to Home Assistant as speech instead of the microphones | root only |
| `/tmp/nspanel-ring-dump` | every piece of speech heard over a ring also saved as a WAV (the last 20) | private speech in RAM |
| `/tmp/nspanel-aec-cold`, `/tmp/nspanel-aec-warm` | echo canceller modes for measurements | root only |
| `/tmp/nspanel-gpu-off` | the image's software-rendered app instead of the GPU app (`S90nspanel-app`) | root only |
| `/tmp/nspanel-proximity-test` | proximity test view (`docs/bringup.md`) | root only |
| `/tmp/nspanel-touch-test` | touch test view (`docs/bringup.md`) | root only |
| `NSPANEL_API_DEBUG` | API message log | root only |
| `NSPANEL_AEC_DUMP` | the microphones and the echo canceller's output written to a file | root only; the speech it records stays in the file; nothing is recorded while the microphone is muted |
| `NSPANEL_FRAME_STATS`, `NSPANEL_NO_WEB`, `NSPANEL_NO_VOICE`, `PANELOS_GLASS` | frame times in the log; no web view; no microphones, wake words or player; the rain-on-glass GPU pass off | root only |
| `NSPANEL_WEB_STATS`, `NSPANEL_DEV_DARK`, `WEBKIT_*` | web view frame timing in the log; backlight kept off; WebKit's own switches passed to the web helper | root only |
| `/mnt/persist/panelos-web-data/console-log` | the dashboard web view writes the page's console messages to the log | dashboard details in `/var/log/messages` (RAM) |
| debugfs, `/dev/mem`, magic SysRq, unsigned kernel modules | kernel debugging | root only |
| Bring-up tools: `i2c-tools` (with `/dev/i2c-*`, `CONFIG_I2C_CHARDEV`), `evtest`, ALSA's `amixer`, `aplay` and `speaker-test` in the image; `cog` in the web bundle | reading and driving the hardware by hand (I2C devices, touch events, sound); a second browser | root only |

Switching these on needs root on the panel, which needs the SSH key. The log lines need nothing switched on: they are written all the time, and through `S98logforward` they reach the serial console (`loglevel=7`) and pstore, where physical access reads them without root.

## Not covered

- **No verified boot.** The stock U-Boot starts whatever is in the boot partition; physical access (BOOT button and USB) or root on the panel means full control. The UI and web bundles are not signed either.
- The panel app runs as root, and so does the web view, without a sandbox (see "Web view").
