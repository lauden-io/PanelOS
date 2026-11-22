# Going back and restoring (Sonoff NSPanel Pro 86)

This guide helps when you want the stock firmware back, or when the panel does not start. It uses the computer and the backup from the [install guide](install.md); type the commands the same way (see "How to type the commands" there).

First open the Ubuntu window (or your Linux terminal), become root and set the panel's values again. Replace the examples; `BACKUP` is the folder that step 6 of the install guide created for this panel (`ls /root/nspanel-backups` lists them):

```
sudo -i
```

```
PANEL=<panel IP address>
```

```
BACKUP=/root/nspanel-backups/<folder of this panel>
```

> **A backup belongs to one panel only.** Never write a backup to another panel: it carries that panel's identity (MAC addresses, keys, serial number).

Which case is yours?

- **A.** PanelOS runs, and you want the stock firmware back.
- **B.** The stock firmware runs, and PanelOS should be removed completely.
- **C.** The panel does not start at all (black screen for minutes, after unplugging and plugging in again too).
- **D.** PanelOS runs, the stock Android was removed (install guide, step 14), and you want the stock firmware back.

## A. From PanelOS back to the stock firmware

```
ssh -n -i /root/.ssh/nspanel_dev -o StrictHostKeyChecking=accept-new -o UserKnownHostsFile=/root/.ssh/panelos_known_hosts root@$PANEL "nspanel-bootmode android && nohup reboot >/dev/null 2>&1 </dev/null &"
```

The answer starts with `default boot: normal`. The panel restarts and the stock firmware (with the Sonoff logo) comes up. It also restores its own recovery partition, so PanelOS is gone afterwards. Developer mode and ADB stay on.

## B. The stock firmware runs: remove the rest of PanelOS

Nothing to do: every time the stock firmware starts, it rewrites the recovery partition with its own content. If you want to be sure, write the backed-up recovery partition back over ADB:

```
adb connect $PANEL:5555
```

```
cd /root/PanelOS && tools/flash_recovery_adb.sh $PANEL:5555 $BACKUP/recovery.img
```

It ends with `recovery partition written and verified`.

## C. The panel does not start: restore over USB

This needs the panel opened, a micro-USB data cable, and the computer. It works even when the panel shows nothing, as long as the bootloader on the panel is intact (PanelOS never changes it).

What you do: start the panel in its USB download mode with the BOOT button, and write the backed-up **misc** partition back. That clears PanelOS's "start PanelOS" setting, so the panel starts from its boot partition: the stock firmware, which then repairs the recovery partition itself, or, once the stock Android is removed (install guide, step 14), the second PanelOS copy there.

### 1. Open the panel

1. **Switch off the circuit of the panel at the fuse box.** The wall part carries mains voltage.
2. Take the display unit off the wall part (it gets its low voltage through an 8-pin connector in the middle of its back; see the teardown photos linked in [hardware.md](hardware.md)).
3. Open the display unit as [hardware.md](hardware.md) describes ("Board and opening the panel"): two screws in the top corners of the back cover, then pry the cover off from the notch on the top edge.
4. Find the **micro-USB port** (marked `OTG`, bottom edge of the board) and the **BOOT** button (right side, next to the `LCD` connector).

### 2. Start the download mode

1. Hold down the **BOOT** button.
2. While holding it, plug the micro-USB cable into the `OTG` port and the computer. The panel gets its power from the USB cable; nothing else needs to be connected.
3. Release the button after about 3 seconds.

### 3. Give the panel to Linux (Windows only)

On Windows, the USB device has to be passed into the Ubuntu window. On Linux, skip to part 4.

1. Open **PowerShell as administrator** (Start → type `PowerShell` → right-click → Run as administrator) and install the USB tool (only the first time):

   ```
   winget install usbipd
   ```

   Close and reopen the administrator PowerShell after the installation.
2. List the USB devices:

   ```
   usbipd list
   ```

   Find the line with `2207:330d` (the panel in download mode) and note its **BUSID** at the start of the line, for example `2-1`.
3. Share it (only the first time; replace `<BUSID>`):

   ```
   usbipd bind --busid <BUSID>
   ```

4. Make sure the Ubuntu window is open, then pass the device to it:

   ```
   usbipd attach --wsl --busid <BUSID>
   ```

### 4. Write the misc partition back

In the Ubuntu window (as root), install the Rockchip USB tool (only the first time):

```
apt install -y rkdeveloptool
```

Check that the panel is visible:

```
rkdeveloptool ld
```

The answer contains `Loader`. If it says no device found, repeat part 2 (and part 3 on Windows).

Write the backed-up misc partition back to its place (sector 32768):

```
rkdeveloptool wl 32768 $BACKUP/misc.img
```

Restart the panel:

```
rkdeveloptool rd
```

### 5. Put the panel back together

Unplug the USB cable, close the display unit, put it back on the wall part, and switch the circuit on again. The stock firmware starts (Sonoff logo). Without the stock Android, the second PanelOS copy starts instead. From there, case D brings the stock firmware back; or write a working image to the recovery partition (install guide, "Updating later") and, before the restart, make it the default again with the first command of the install guide's step 9.

If it still does not start, write the backed-up recovery partition too (download mode again, parts 2–4), with this command instead of the misc one:

```
rkdeveloptool wl 204800 $BACKUP/recovery.img
```

Without the stock Android, if the PanelOS copy on the boot partition does not start either, write the backed-up boot partition instead (download mode again, parts 2–4). It puts the stock Android's boot partition back, so the stock firmware should start again and repair the recovery partition. This way has not been tried on a panel yet.

```
rkdeveloptool wl 139264 $BACKUP/boot.img
```

Do not write the `uboot`, `trust` or other partitions unless you know exactly why: the USB download mode itself depends on them.

## D. Android was removed: back to the stock firmware

PanelOS writes the stock boot and recovery partitions back from this panel's backup, clears the old userdata partition and restarts into the stock firmware:

```
cd /root/PanelOS && tools/remove_android.sh $PANEL restore $BACKUP
```

It asks first, checks every partition after writing it, and stops before the restart if something does not match (PanelOS keeps running then; run the command again). The stock firmware should then start like after a factory reset: its own settings and the eWeLink pairing gone, developer mode and ADB to be turned on again (install guide, step 1). This way back has not been tried on a panel yet.

## Test the restore before you need it

Do this right after the backup (install guide, step 6), while the panel still runs the stock firmware. Both tests write only the recovery partition, with exactly the content it already has. The stock firmware starts without the recovery partition, and it restores its own recovery at every start anyway, so nothing is at risk.

### Quick test over ADB (the panel stays closed)

With `PANEL` and `BACKUP` set (top of this page):

```
adb connect $PANEL:5555
```

```
cd /root/PanelOS && tools/flash_recovery_adb.sh $PANEL:5555 $BACKUP/recovery.img
```

It must end with `recovery partition written and verified`. This proves that the backup files are good and that the panel can be written over ADB, which is the way back while the stock firmware still starts.

### Full test over USB (the panel has to be opened)

This proves the rescue path of case C before you need it.

1. Open the panel and start the download mode: case C, parts 1 and 2.
2. On Windows, pass the panel to Linux: case C, part 3.
3. In the Ubuntu window, install the tool if it is not there yet:

   ```
   apt install -y rkdeveloptool
   ```

4. Run the test. It reads the `uboot` and `recovery` partitions back and compares them with the backup, writes the backed-up recovery partition back, reads it again, and restarts the panel:

   ```
   cd /root/PanelOS && tools/restore_test.sh $BACKUP --write
   ```

   Every check must say `OK`, and the last line is `done`. If a line says `MISMATCH`, the backup does not match the panel: do not install PanelOS, and make a new backup first.
5. Unplug the USB cable and put the panel back together (case C, part 5). The stock firmware starts.

## Technical notes

- **USB download mode**: holding BOOT while connecting USB starts the vendor U-Boot's rockusb Loader mode (USB ID `2207:330d`). If U-Boot itself were broken, only the boot ROM's mask ROM mode would be left, which needs a PX30 loader blob (`rkdeveloptool db`); PanelOS therefore never touches `uboot`, `trust` or the idbloader area.
- **Sector numbers** are the start sectors in [hardware.md](hardware.md) (eMMC partition map, measured on firmware 3.0.0; check your own unit). Rockusb sector numbers are the same as Android's (LBA 16384 starts with the `LOADER` header of uboot). Reading: `rkdeveloptool rl <start_sector> <length> <file>`; in the Ubuntu package (1.32+pine64) the start is in sectors but the **length is in bytes**.
- **Backups over ADB** (`tools/backup_partitions.sh`) copy each partition to `/data/local/tmp` on the panel with `cat`, pull it with `adb pull` and compare the md5 on both sides. Two pitfalls: `adb exec-out "su -c 'cat /dev/block/...'" > file.img` corrupts binary data on Windows (LF becomes CRLF), and the stock toolbox `dd` rejects `bs=1M`.
- **Soft restore** while the stock firmware runs: unmounted partitions can be written back with root, `su -c 'cat <partition>.img > /dev/block/by-name/<partition>'`. Do not rewrite mounted partitions (`system`, `vendor`) or the running `boot` this way.
