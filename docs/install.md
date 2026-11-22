# Installing PanelOS on the Sonoff NSPanel Pro 86

This guide takes you from a stock NSPanel Pro to PanelOS step by step. You do not need to know Linux.

> **Read this first.** Installing replaces the panel's software. It can go wrong, and turning on developer mode (step 1) ends official Sonoff updates for good. You need a working backup (step 6) before anything is written. You do everything at your own risk.

## What you need

- A **Sonoff NSPanel Pro, 86 model** (the square 86 × 86 mm one), on its stock firmware, set up in the eWeLink app and **connected to your Wi-Fi**. PanelOS takes over this Wi-Fi network: with several saved networks, the one the panel is connected to. Password-protected (WPA/WPA2 personal), open and hidden networks work; WEP and enterprise (802.1X) networks are not taken over. It has been tested on stock firmware 3.0.0.
- A **computer**: Windows 10/11, Linux, or a Mac (see [Mac](#mac)), with about **60 GB free disk space** (80 GB for the Mac virtual machine) and internet access. The first build takes a long time (it builds a whole Linux system, and a web browser for the dashboard mode: several hours); later builds take a few minutes.
- **Home Assistant** on the same network, and a phone with a camera.
- The **IP address of Home Assistant** (for example `192.168.1.10`; in Home Assistant: Settings → System → Network). The panel cannot use names like `homeassistant.local`.

## How to type the commands

Everything from step 3 on happens in one **terminal window** on the computer: the **Ubuntu** window on Windows (step 3 installs it), a terminal on Linux, or on a Mac the Terminal window connected to the Linux virtual machine (step 3, Mac).

- Copy a command from this page and paste it into the window: **right-click** in the Ubuntu window, **Ctrl+Shift+V** in a Linux terminal, **Cmd+V** in the Mac Terminal. Then press **Enter**.
- Wait until the command has finished: the window shows the prompt again, a line ending in `#`, for example `root@PC:~#`. Only then paste the next one.
- Paste the commands **one at a time**, in the order of this guide. Each grey box is one command.
- Replace anything in `<angle brackets>` with your own value and leave out the brackets.
- Some lines only store a value for the later commands, for example `PANEL=192.168.1.50`. They print nothing. The value is kept while the window stays open; if you close the window, open it again, type `sudo -i`, and repeat these lines (they are marked **"Set again after reopening"**).

## 1. Turn on developer mode and ADB on the panel

ADB is the service the computer uses to install PanelOS. On the stock firmware it is switched on in the eWeLink app on your phone:

1. Open the **eWeLink** app and open the NSPanel Pro.
2. Tap the **three dots** in the top right corner to open the device settings.
3. Scroll down to **Device ID** and **tap it repeatedly** (about eight times) until the app says developer mode is on.
4. Open the new **Developer mode** item and switch on **ADB**. Accept the agreement: this cannot be undone, and the panel will not get official firmware updates any more.

Developer mode is not available on every firmware version, and the menus can differ. These community guides describe the steps, workarounds and pitfalls:

- [blakadder: Sideload apps to Sonoff NSPanel Pro](https://blakadder.com/nspanel-pro-sideload/)
- [Home Automation Guy: Home Assistant on a Sonoff NSPanel Pro](https://www.homeautomationguy.io/blog/home-assistant-on-a-sonoff-nspanel-pro)
- [Home Assistant Community: NSPanel Pro – custom Android apps](https://community.home-assistant.io/t/nspanel-pro-custom-android-apps-working/459563)
- [seaky/nspanel_pro_roottool_apk](https://github.com/seaky/nspanel_pro_roottool_apk)

PanelOS needs ADB **with root** (the `su` command). Step 5 checks it.

## 2. Find the panel's IP address

The eWeLink app shows it in the device settings, and your router lists it among the connected devices. Write it down (for example `192.168.1.50`). Give the panel a fixed address in your router (DHCP reservation), so it does not change later.

## 3. Prepare the computer

PanelOS is built on Linux.

### Windows

Windows runs Linux in WSL, a Linux system inside Windows.

1. Click Start, type `PowerShell`, right-click **Windows PowerShell** and choose **Run as administrator**.
2. Paste this and press Enter:

   ```
   wsl --install -d Ubuntu-24.04
   ```

3. Restart the computer when it asks you to.
4. Open **Ubuntu 24.04** from the Start menu. The first time it asks for a user name and a password: choose any (you need the password in a moment). This window is the **Ubuntu window** of this guide.

### Linux

Use Ubuntu 24.04 or a similar distribution and open a terminal.

### Mac

On a Mac the build runs in a Linux virtual machine with the free UTM app. The build needs an x86-64 (Intel) Linux system:

- on an **Intel Mac** UTM runs it at full speed;
- on a **Mac with Apple silicon** (M1 and later) UTM has to emulate an Intel processor, which works but is slow: the first build can take many hours. Native support for Apple silicon is planned.

This route has not been tested yet; the UTM screens may look slightly different.

1. Download and install UTM from [mac.getutm.app](https://mac.getutm.app/).
2. Download the **Ubuntu Server 24.04 LTS** installer for **amd64 (x86-64)** from [ubuntu.com/download/server](https://ubuntu.com/download/server) (a file like `ubuntu-24.04.x-live-server-amd64.iso`).
3. In UTM select **+** → **Create a New Virtual Machine**:
   - Intel Mac: **Virtualize**; Apple silicon: **Emulate**.
   - Operating system: **Linux**.
   - Boot ISO image: **Browse** and select the downloaded file. On Apple silicon set the architecture to **x86_64**.
   - Memory: at least **8192 MB**; CPU cores: as many as offered (at least 4).
   - Storage: at least **80 GB**.
   - Save, then start the virtual machine with the **play** button.
4. Install Ubuntu with the default answers. Choose a user name and password, and tick **Install OpenSSH server** when it is offered. When the installation asks to restart, remove the installer first (in UTM: the CD/DVD icon → **Clear**), then restart.
5. Log in in the virtual machine window and find its address with this command (it prints the address, for example `192.168.64.5`; if it prints more than one, use the first):

   ```
   hostname -I
   ```

6. On the Mac, open **Terminal** (Applications → Utilities) and connect to the virtual machine (replace the user and the address):

   ```
   ssh <user>@<address of the virtual machine>
   ```

   Answer `yes` to the question about the fingerprint and type the password. From now on this Terminal window is the window of this guide: paste with **Cmd+V**, and continue with "Install the tools" below.

### Install the tools

In the Ubuntu window (or Linux terminal), become the administrator (root). It asks for the password you chose:

```
sudo -i
```

The prompt now ends in `#`. Install the tools the build needs:

```
apt update && apt install -y build-essential bc bison flex cpio rsync unzip wget curl git file patch python3 libssl-dev libelf-dev libncurses-dev device-tree-compiler gcc-aarch64-linux-gnu lz4 zstd openssh-client adb
```

## 4. Download PanelOS and its sources

```
git clone https://github.com/lauden-io/PanelOS.git /root/PanelOS
```

```
cd /root/PanelOS && tools/fetch_sources.sh
```

The second command downloads the Linux kernel and Buildroot (a few GB). It ends with a line like `kernel: 7.3.1  buildroot: 2026.08.1`.

## 5. Connect to the panel and check root

Store the panel's IP address and a short name for the panel. The name is the panel's own name from now on: its backups are named after it (step 6), and the panel is known by it in Home Assistant (step 9). Use lower-case letters `a`–`z`, digits and hyphens, at most 31 characters, for example `kitchen`; every panel gets a different one, and no other ESPHome device in Home Assistant may have it. **Set again after reopening.**

```
PANEL=<panel IP address>
```

```
NAME=<short name of the panel>
```

Check the name. It must print `name ok`:

```
echo "$NAME" | grep -Eqx '[a-z0-9]([a-z0-9-]{0,29}[a-z0-9])?' && echo "name ok" || echo "not a valid name"
```

Connect to the panel:

```
adb connect $PANEL:5555
```

It should say `connected to …`. If the panel asks on its screen whether to allow the connection, allow it. Then check root:

```
adb -s $PANEL:5555 shell su -c id
```

The answer must start with `uid=0(root)`. If it does not, stop here: without root PanelOS cannot be installed (see the guides in step 1).

## 6. Back up the panel

Every panel gets its own backup folder, named after the panel and the date, so backups of several panels never overwrite each other. **Set again after reopening.**

```
BACKUP=/root/nspanel-backups/$NAME-$(date +%Y-%m-%d)
```

```
cd /root/PanelOS && tools/backup_partitions.sh $PANEL:5555 $BACKUP
```

This copies the panel's partitions (about 2.5 GB; it takes a while) and checks every copy; it ends with `done: ok=16 bad=0` (`bad` must be 0). Then copy the backups out of Linux too.

- Windows, into your Documents folder (replace `<your Windows user>`):

  ```
  cp -r /root/nspanel-backups "/mnt/c/Users/<your Windows user>/Documents/"
  ```

- Linux or a virtual machine: copy `/root/nspanel-backups` to a USB drive or another computer.

Keep the backups private: they contain each panel's own identity (MAC addresses, keys, serial number). **A backup belongs to one panel only: never write it to another panel.** [restore.md](restore.md) explains how to use it.

### Check that the backup can be restored (optional, strongly recommended)

A backup only helps if it can be written back. Test it now, while the panel still runs the stock firmware and nothing is at stake. [restore.md](restore.md#test-the-restore-before-you-need-it) describes two tests:

- **Quick test** (one command, the panel stays closed): writes the backed-up recovery partition back over ADB and checks it.
- **Full test** (the panel has to be opened): checks the USB rescue path, the one you need when the panel does not start at all.

## 7. Build PanelOS

PanelOS comes in three parts, built one after the other:

- the **system image**: Linux and a simple version of the PanelOS app;
- the **web bundle**: the web browser of the dashboard mode and the graphics libraries;
- the **UI bundle**: the full PanelOS app, drawn on the graphics chip, with the pictures, fonts and sounds of its pages.

First the system image:

```
cd /root/PanelOS && tools/build_image.sh
```

When it is done, the last line is `done: /tmp/nspanel_out/recovery-dev.img`. The build also creates an SSH key in `/root/.ssh/nspanel_dev`: it is your key to log in to your panels later, so keep it.

The build takes one file from the backup of step 6: the stock device tree from the `resource` partition. The panel's bootloader needs it to show the boot logo and to load PanelOS in about 2 seconds.

Then the web bundle. This is the long step: the first time it builds a whole web browser, which takes several hours.

```
cd /root/PanelOS && tools/build_web.sh
```

When it is done, the last line shows the file `/root/nspanel/br-web/images/rootfs.squashfs`.

Then the UI bundle:

```
cd /root/PanelOS && tools/build_ui_bundle.sh
```

When it is done, the last line is `done: /root/nspanel/out/panelos-ui.squashfs`.

You build once; the same image and bundles go on every NSPanel Pro 86 you have.

## 8. Install PanelOS

How it works: PanelOS goes into the panel's **recovery** partition, and the panel starts it once. The stock firmware stays on the panel. In step 9 you tell the panel to start PanelOS every time. This matters, because whenever the stock firmware starts, it rewrites the recovery partition with its own content and PanelOS is gone.

```
cd /root/PanelOS && tools/flash_recovery_adb.sh $PANEL:5555 /tmp/nspanel_out/recovery-dev.img --boot
```

The command writes the image, checks it, and restarts the panel. On the panel you see:

1. after a few seconds the **PanelOS logo** (the stock bootloader loads PanelOS), then a short black moment,
2. the **PanelOS boot animation**, which waits until Wi-Fi is up,
3. the setup screen **"Connect to Home Assistant"** (step 10).

You do this step **once**.

If the panel starts the stock firmware again (the Sonoff screen) instead of PanelOS, PanelOS found no network within 90 seconds and went back by itself. Check in the stock firmware that the panel is connected to your Wi-Fi. Then run the command of this step again.

## 9. Make PanelOS start every time and put its bundles on the panel

Do this while PanelOS is running (after the animation):

```
ssh -n -i /root/.ssh/nspanel_dev -o StrictHostKeyChecking=accept-new -o UserKnownHostsFile=/root/.ssh/panelos_known_hosts root@$PANEL nspanel-bootmode open
```

The answer starts with `default boot: open system`. From now on the panel starts PanelOS every time, also after a power cut.

The first SSH command stores the panel's SSH host key in `/root/.ssh/panelos_known_hosts`; later commands check it. The panel keeps the key across reboots and updates; should it ever change on purpose (e.g. after restoring the stock firmware), remove the old one with `ssh-keygen -R $PANEL -f /root/.ssh/panelos_known_hosts`.

Then put the two bundles of step 7 on the panel:

```
cd /root/PanelOS && tools/install_bundles.sh $PANEL
```

It sends both bundles over Wi-Fi (about 250 MB, a few minutes), checks them on the panel, and restarts the PanelOS app with them. The panel keeps running while they are sent. At the end it says `the app runs from the UI bundle, on the GPU`, `dashboard mode: the web bundle is mounted` and `done`, and the panel shows the setup screen again.

If it stops with a message, nothing has changed on the panel unless the message says otherwise; run the command again (an upload that already got through is not sent again).

Then give the panel its name (step 5). The command writes it on the panel and restarts the panel; PanelOS comes back with the setup screen after about a minute:

```
ssh -n -i /root/.ssh/nspanel_dev -o StrictHostKeyChecking=accept-new -o UserKnownHostsFile=/root/.ssh/panelos_known_hosts root@$PANEL "echo $NAME > /mnt/persist/nspanel/panel_name && nohup reboot >/dev/null 2>&1 </dev/null &"
```

The panel's settings show the name under **About** as its **Panel ID**. A panel without a name is `nspanel-pro`: panels installed before names keep it, with all their devices and entities in Home Assistant. To rename a panel later, see [home_assistant.md](home_assistant.md#renaming-a-panel).

## 10. Tell the panel where Home Assistant is

On the panel's setup screen **"Connect to Home Assistant"**:

- If your Home Assistant is listed (the panel found it on the network), tap it.
- Otherwise type its IP address on the keypad, plus `:8123` if your Home Assistant uses that port (installations from before 2026.8 and Docker installations do; Home Assistant OS installed since 2026.8 uses port 80). Without a port the panel tries 8123 first, then 80. Tap **Connect**.

The panel checks that Home Assistant answers at that address, saves it, and shows a **QR code** a moment later. If it says that Home Assistant did not answer, check the IP address and the port.

Alternatively, from the computer (with `HA=http://<Home Assistant IP address>:8123`):

```
ssh -n -i /root/.ssh/nspanel_dev -o StrictHostKeyChecking=accept-new -o UserKnownHostsFile=/root/.ssh/panelos_known_hosts root@$PANEL "echo '{\"ha_url\": \"$HA\"}' > /mnt/persist/nspanel/app.json && nohup reboot >/dev/null 2>&1 </dev/null &"
```

## 11. Log in to Home Assistant

1. Scan the QR code with the phone's camera (Android: the camera app or Google Lens; iPhone: the Camera app), and open the link it shows.
2. The Home Assistant login page opens. It always asks for a user name and password, also when the phone is already logged in to Home Assistant, so log in with the user the panel should use (a separate, non-administrator Home Assistant user works well).
3. The panel connects by itself and shows your default dashboard. You can pick another dashboard on the panel: gear icon → Dashboard.

## 12. Add the panel to Home Assistant

1. If Home Assistant and the panel are in the same network segment, Home Assistant finds the panel by itself (mDNS): **Settings → Devices & services** shows it under Discovered; tap **Add**. Otherwise: **Settings → Devices & services → Add integration → ESPHome**.
2. Host: the panel's IP address (pre-filled when discovered). Port: `6053`. Leave the encryption key empty: Home Assistant gives the panel a key by itself right after adding it and connects encrypted from then on.
3. The panel appears as a device with its settings, sensors and the voice assistant (Assist satellite). Choose the wake words and the assistant on the device page.

### The PanelOS integration

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

Then **Settings → Devices & services → Add integration → PanelOS**, Panel ID: the panel's name from step 5. Further panels are added on the PanelOS page (**Add a panel**). The panel's pages, looks, voice and display settings are then set on the PanelOS page. More: [home_assistant.md](home_assistant.md).

## 13. Check

Unplug the panel's power (or switch off its circuit) for 10 seconds and switch it on again. After the logo and the animation, the panel must show your dashboard again.

## 14. Remove the stock Android (optional)

PanelOS does not need the stock Android. While Android is on the panel, it is the way back to the stock firmware ([restore.md](restore.md), case A) and the safety net of test boots. Once PanelOS has run well for a while, its partitions can be taken over, all over Wi-Fi, with the panel closed:

- the **boot** partition gets a second copy of PanelOS. A normal boot, and so the safety net of a test boot, then starts PanelOS instead of Android;
- the old **userdata** partition (about 3.5 GiB) becomes PanelOS's storage for larger content (`/mnt/data`).

The other Android partitions stay as they are, unused. The way back to the stock firmware is then the backup of step 6, written over Wi-Fi while PanelOS runs ([restore.md](restore.md), case D).

Before you start: the backup of step 6 is on the computer (best with its restore test), and PanelOS has run for some days without trouble.

> **Tried on one panel so far** (2026-11-15, steps 1–4): PanelOS started from the boot partition and was on the network after 8 s, and after formatting `/mnt/data` was mounted at the next start by itself. Read the risks first.

**Risks, and why they are small.** What speaks for it, checked on a backup of the stock firmware 3.0.0: the stock boot and recovery images have the same layout (Android boot image version 0, page 2048, each with its own kernel and the same second stage); the stock bootloader's start command (`boot_android`, then `bootrkp`, then the distro boot) loads both partitions with the same code; it has no verified boot (no AVB, no signature check of the boot partition) and passes no `skip_initramfs`; the image that goes to the boot partition is the very one that already runs from recovery, byte for byte, and it fits (30.7 MB of 32 MiB).

| Risk | When | What happens | Way out |
|---|---|---|---|
| PanelOS does not start from the boot partition | step 3, the one test start | the panel keeps restarting the boot partition | unplug for 10 s; if it still does not start, the USB route of [restore.md](restore.md) case C: the panel has to be opened for its BOOT button. This is the one risk that can need opening the panel. |
| Power cut while writing the boot partition | step 2 (a few seconds) | the boot partition is half written; the default boot still starts PanelOS from recovery | run step 2 again |
| Power cut while formatting | step 4 | the new data partition is unusable | run step 4 again |
| Both partitions hold a broken image | later, only if a bad image is written to both | no system starts | [restore.md](restore.md) case C (opening the panel). Copy an image to the boot partition only after it has proven itself (see "Updating later"). |
| The stock firmware is gone | from step 2 | the panel cannot go back to Sonoff's firmware by itself | the backup of step 6, written by PanelOS ([restore.md](restore.md) case D); the stock firmware then starts like after a factory reset. Keep the backup safe: without it there is no way back to the stock firmware. |
| A bigger image does not fit | later updates | the second copy cannot be refreshed | `tools/build_image.sh` warns when an image is larger than the boot partition |

Today, while the stock Android is on the panel, a PanelOS that does not start at all needs the same USB route; what step 14 changes is that the stock Android is no longer the fallback, the second PanelOS copy is.

1. See what is on the panel (changes nothing):

   ```
   cd /root/PanelOS && tools/remove_android.sh $PANEL check
   ```

2. Copy the running PanelOS to the boot partition. It asks first, then writes and checks. **From here on the stock Android does not start any more.**

   ```
   tools/remove_android.sh $PANEL copy
   ```

3. Start once from the boot partition. The panel restarts, PanelOS comes up again in about a minute, and the recovery partition becomes the default again:

   ```
   tools/remove_android.sh $PANEL test
   ```

   If the panel does not come back within 5 minutes, unplug its power for 10 seconds. If it still does not start, follow [restore.md](restore.md), case C. This is the only point of this step where the panel may have to be opened.

4. Format the old userdata partition for PanelOS (needs a PanelOS with `mkfs.ext4`, from version 0.0.4). PanelOS mounts it at `/mnt/data` at every start:

   ```
   tools/remove_android.sh $PANEL data
   ```

5. Move the UI bundle to the new, larger partition. This frees room on the small one, where the web bundle stays:

   ```
   tools/install_bundles.sh $PANEL ui
   ```

## More panels

Do steps 1–2, 5–6 and 8–13 for each further panel. Steps 3, 4 and 7 are not needed again: the computer is ready, and the same image and bundles fit every NSPanel Pro 86. In step 5 set `PANEL` and `NAME` for the new panel: its backup goes into its own folder, and step 9 gives it its own name, so several panels work side by side in one Home Assistant.

## Updating later

Until PanelOS can update itself (planned: updates from Home Assistant's Settings → Updates, without a computer), an update is built and written from the computer, the same way as the install.

1. In the Ubuntu window, become root (`sudo -i`) and build the new version once. One build serves all your panels:

   ```
   cd /root/PanelOS && git pull && tools/build_image.sh
   ```

   The web bundle changes seldom, and its build takes long. Build it again only when the release notes ([CHANGELOG.md](../CHANGELOG.md), "Upgrading") ask for a new web bundle:

   ```
   cd /root/PanelOS && tools/build_web.sh
   ```

   Then the UI bundle (always after the web bundle):

   ```
   cd /root/PanelOS && tools/build_ui_bundle.sh
   ```

2. For **each panel**, set its address, write the new version and restart it. The panel keeps running PanelOS while it is written; the new version starts after the restart, and all settings (Wi-Fi, Home Assistant login, choices) stay.

   ```
   PANEL=<panel IP address>
   ```

   ```
   cd /root/PanelOS && tools/flash_recovery_ssh.sh $PANEL /tmp/nspanel_out/recovery-dev.img
   ```

   ```
   ssh -n -i /root/.ssh/nspanel_dev -o StrictHostKeyChecking=accept-new -o UserKnownHostsFile=/root/.ssh/panelos_known_hosts root@$PANEL "nohup reboot >/dev/null 2>&1 </dev/null &"
   ```

   When PanelOS is back (about a minute), put the new bundles on it. A bundle that is already on the panel is skipped, so an unchanged web bundle is not sent again:

   ```
   cd /root/PanelOS && tools/install_bundles.sh $PANEL
   ```

   Then the next panel: set `PANEL` again and repeat these four commands.

3. On a panel without the stock Android (step 14) the boot partition keeps the previous version as the fallback. Once the new version has proven itself, copy it there too: `tools/remove_android.sh $PANEL copy`.

## Going back to the stock firmware

See [restore.md](restore.md), case A.

## If something goes wrong

- **The panel shows the Sonoff screen / stock firmware again:** PanelOS was not made the default (step 9) or found no network. Repeat from step 8.
- **The setup screen says Home Assistant did not answer:** check the IP address and the port (`:8123` or none) in Home Assistant under Settings → System → Network.
- **"Waiting for the network…" or "Home Assistant not reachable, retrying…" in the status line:** the panel keeps trying by itself, no restart needed; check the Wi-Fi and that Home Assistant runs.
- **The screen stays black for minutes:** unplug the power for 10 seconds. If it still does not start, follow [restore.md](restore.md), case C.
- **`tools/install_bundles.sh` says there is not enough room:** until step 14, both bundles (about 250 MB) share the small persistent partition (384 MiB) with the settings and the browser's cache. This shows what takes the room:

  ```
  ssh -n -i /root/.ssh/nspanel_dev -o UserKnownHostsFile=/root/.ssh/panelos_known_hosts root@$PANEL "du -sh /mnt/persist/*"
  ```

  After step 14 the UI bundle goes to the large data partition.
- **`tools/install_bundles.sh` ends with "the image's app runs":** the line under it says why. When the app from the UI bundle ended right after its start three times, the panel uses the simple app until its next restart; the app's messages:

  ```
  ssh -n -i /root/.ssh/nspanel_dev -o UserKnownHostsFile=/root/.ssh/panelos_known_hosts root@$PANEL "grep nspanel-app /var/log/messages | tail -30"
  ```
