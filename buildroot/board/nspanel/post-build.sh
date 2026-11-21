#!/bin/sh
# Buildroot post-build hook.
# Installs the SSH public key for development logins, kept outside this repository:
#   NSPANEL_SSH_PUBKEY_FILE=/path/to/key.pub make ...
set -e
TARGET="$1"

if [ -n "${NSPANEL_SSH_PUBKEY_FILE:-}" ] && [ -f "$NSPANEL_SSH_PUBKEY_FILE" ]; then
	install -d -m 700 "$TARGET/root/.ssh"
	install -m 600 "$NSPANEL_SSH_PUBKEY_FILE" "$TARGET/root/.ssh/authorized_keys"
	echo "post-build: installed SSH public key"
else
	echo "post-build: no NSPANEL_SSH_PUBKEY_FILE, SSH login will not be possible"
fi

# Root logs in with the SSH key only: no password, also not on the serial
# console (Buildroot leaves it empty).
sed -i 's/^root:[^:]*:/root:*:/' "$TARGET/etc/shadow"

# One system font is enough: the panel app embeds its own fonts; fontconfig
# (needed by the app's font backend) only must find at least one font.
if [ -d "$TARGET/usr/share/fonts/dejavu" ]; then
	find "$TARGET/usr/share/fonts/dejavu" -type f ! -name DejaVuSans.ttf -delete
fi

# FlatBuffers' compiler comes with the library TensorFlow Lite needs; the panel never runs it.
rm -f "$TARGET/usr/bin/flatc"
