#!/bin/sh
# Tests nspanel-android-wifi, the Wi-Fi takeover's parser, on the made-up
# WifiConfigStore.xml files in tools/wifi_import_test/.
#
#   tools/test_wifi_import.sh [parser]
#   BUSYBOX=/path/to/busybox tools/test_wifi_import.sh [parser]
#
# Each <case>.xml has its expected standard output in <case>.out and, when the
# import must succeed, the expected config in <case>.conf (no .conf: the
# parser must fail and write nothing).
#
# The parser is the one in the overlay next to this script, else
# nspanel-android-wifi on PATH (on the panel), or the path given. It runs with
# the sh, awk, sed and printf on PATH. With BUSYBOX set, the test runs again
# under that busybox's sh with all its applets first on PATH: on a PC, a host
# build of the panel's busybox (the Buildroot busybox source with its .config,
# CONFIG_CROSS_COMPILER_PREFIX emptied, `make busybox`) runs the same ash, awk
# and sed code as the panel.
#
# wpa_passphrase is the installed one (on the panel), else a stand-in in
# Python 3 with the same checks and the same key (PBKDF2-SHA1, 4096 rounds);
# case 13 holds the IEEE 802.11i test vector, so a wrong stand-in shows.

here=$(cd "$(dirname "$0")" && pwd)

if [ -n "$BUSYBOX" ] && [ -z "$WIFI_TEST_BUSYBOX_DIR" ]; then
	bb=$(cd "$(dirname "$BUSYBOX")" && pwd)/$(basename "$BUSYBOX")
	WIFI_TEST_BUSYBOX_DIR=$(mktemp -d)
	for applet in $("$bb" --list); do
		ln -s "$bb" "$WIFI_TEST_BUSYBOX_DIR/$applet"
	done
	export WIFI_TEST_BUSYBOX_DIR
	PATH="$WIFI_TEST_BUSYBOX_DIR:$PATH" "$bb" sh "$here/$(basename "$0")" "$@"
	status=$?
	rm -rf "$WIFI_TEST_BUSYBOX_DIR"
	exit $status
fi

cases="$here/wifi_import_test"
parser=$1
if [ -z "$parser" ]; then
	parser="$here/../buildroot/board/nspanel/rootfs_overlay/usr/sbin/nspanel-android-wifi"
	[ -f "$parser" ] || parser=$(command -v nspanel-android-wifi)
fi
if [ ! -f "$parser" ]; then
	echo "parser not found: $parser"
	exit 1
fi

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
mkdir "$tmp/bin"

if command -v wpa_passphrase > /dev/null; then
	passphrase="$(command -v wpa_passphrase)"
else
	python=$(command -v python3)
	if [ -z "$python" ]; then
		echo "neither wpa_passphrase nor python3 found"
		exit 1
	fi
	printf '#!%s\n' "$python" > "$tmp/bin/wpa_passphrase"
	cat >> "$tmp/bin/wpa_passphrase" << 'EOF'
# Stand-in for wpa_passphrase (wpa_supplicant 2.12): same checks, same output.
import hashlib, os, sys
if len(sys.argv) < 2:
    print("usage: wpa_passphrase <ssid> [passphrase]")
    sys.exit(1)
ssid = os.fsencode(sys.argv[1])
if len(sys.argv) > 2:
    pw = os.fsencode(sys.argv[2])
else:
    sys.stderr.write("# reading passphrase from stdin\n")
    pw = sys.stdin.buffer.readline(63).split(b"\n")[0].split(b"\r")[0]
if not 8 <= len(pw) <= 63:
    sys.stderr.write("Passphrase must be 8..63 characters\n")
    sys.exit(1)
if any(c < 32 or c == 127 for c in pw):
    sys.stderr.write("Invalid passphrase character\n")
    sys.exit(1)
key = hashlib.pbkdf2_hmac("sha1", pw, ssid, 4096, 32).hex().encode()
sys.stdout.buffer.write(b'network={\n\tssid="' + ssid + b'"\n\t#psk="' + pw + b'"\n\tpsk=' + key + b"\n}\n")
EOF
	chmod 755 "$tmp/bin/wpa_passphrase"
	passphrase="Python stand-in"
	PATH="$tmp/bin:$PATH"
fi

echo "parser:         $parser"
echo "sh:             $(command -v sh)${WIFI_TEST_BUSYBOX_DIR:+ (busybox)}"
echo "awk:            $(command -v awk)"
echo "wpa_passphrase: $passphrase"

pass=0
fail=0
for xml in "$cases"/*.xml; do
	case=$(basename "$xml" .xml)
	conf="$cases/$case.conf"
	rm -f "$tmp/out.conf"
	got=$(sh "$parser" "$xml" "$tmp/out.conf")
	status=$?
	want=$(cat "$cases/$case.out")
	problem=""
	[ "$got" = "$want" ] || problem="output: got [$got], want [$want]"
	if [ -f "$conf" ]; then
		if [ "$status" -ne 0 ]; then
			problem="$problem exit status $status"
		elif ! cmp -s "$tmp/out.conf" "$conf"; then
			problem="$problem config differs"
			diff "$conf" "$tmp/out.conf"
		fi
	else
		[ "$status" -eq 1 ] || problem="$problem exit status $status, want 1"
		[ -e "$tmp/out.conf" ] && problem="$problem a config was written"
	fi
	if [ -z "$problem" ]; then
		pass=$((pass + 1))
		echo "ok    $case"
	else
		fail=$((fail + 1))
		echo "FAIL  $case: $problem"
	fi
done
echo "$pass passed, $fail failed"
[ "$fail" -eq 0 ]
