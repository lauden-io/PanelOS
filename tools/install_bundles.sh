#!/usr/bin/env bash
# Puts the UI bundle and/or the web bundle on a panel that runs PanelOS, over SSH.
#
#   tools/install_bundles.sh <host>          both bundles
#   tools/install_bundles.sh <host> ui       the UI bundle only
#   tools/install_bundles.sh <host> web      the web bundle only
#
# The files (defaults: what tools/build_ui_bundle.sh and tools/build_web.sh make):
#   UI_BUNDLE    /root/nspanel/out/panelos-ui.squashfs
#   WEB_BUNDLE   /root/nspanel/br-web/images/rootfs.squashfs
# Env: NSPANEL_SSH_KEY (default /root/.ssh/nspanel_dev), KNOWN_HOSTS, YES=1 (no questions),
#      DRY_RUN=1 (only shows where the bundles would go and how; changes nothing)
#
# On the panel the UI bundle goes to /mnt/data/panelos-ui.squashfs when the data
# partition is there (tools/remove_android.sh data), else to
# /mnt/persist/panelos-ui.squashfs; the web bundle goes to
# /mnt/persist/panelos-web.squashfs (where S51panelos-ui and S50panelos-web look).
#
# How: each file is uploaded under a temporary name next to its place and
# checked (size and md5) while the app keeps running; a failed upload changes
# nothing. Where the partition has no room for a second copy, the upload goes to
# the data partition and is copied into place later, or, without room there
# either, the old bundle is removed before the upload (it asks first; the panel
# runs the image's app meanwhile). Then a script on the panel, detached so that
# a dropped connection cannot stop it halfway, checks the uploads again, stops
# the app, unmounts the bundles, renames the new files into place (the old or
# the new file is there, never part of one), mounts them and starts the app.
# A bundle that is the same as the one on the panel is skipped.
set -euo pipefail

usage="usage: $0 <host> [ui] [web]"
HOST="${1:?$usage}"
shift
WANT_UI=0
WANT_WEB=0
[ $# -gt 0 ] || set -- ui web
for k in "$@"; do
  case $k in
  ui) WANT_UI=1 ;;
  web) WANT_WEB=1 ;;
  *) echo "$usage" >&2; exit 1 ;;
  esac
done
KINDS=()
[ "$WANT_UI" = 1 ] && KINDS+=(ui)
[ "$WANT_WEB" = 1 ] && KINDS+=(web)

UI_BUNDLE="${UI_BUNDLE:-/root/nspanel/out/panelos-ui.squashfs}"
WEB_BUNDLE="${WEB_BUNDLE:-/root/nspanel/br-web/images/rootfs.squashfs}"
BRWEB="${BRWEB:-/root/nspanel/br-web}"
KEY="${NSPANEL_SSH_KEY:-/root/.ssh/nspanel_dev}"
SSH_OPTS=(-i "$KEY" -o ConnectTimeout=10 -o ServerAliveInterval=5 -o ServerAliveCountMax=6
  -o StrictHostKeyChecking=accept-new -o UserKnownHostsFile="${KNOWN_HOSTS:-$HOME/.ssh/panelos_known_hosts}" -o LogLevel=ERROR)
on_panel() { ssh -n "${SSH_OPTS[@]}" "root@$HOST" "$@"; }
# The same without -n, for sending something on stdin.
push() { ssh "${SSH_OPTS[@]}" "root@$HOST" "$@"; }
die() { echo "$*" >&2; exit 1; }
mb() { echo $(( ($1 + 500000) / 1000000 )); }
kb() { echo $(( ($1 + 1023) / 1024 )); }
# Room left free on a partition after an upload, KB.
MARGIN=4096

########## the files ##########
UNSQUASHFS="$BRWEB/host/bin/unsquashfs"
[ -x "$UNSQUASHFS" ] || UNSQUASHFS=$(command -v unsquashfs || true)
declare -A FILE SIZE SUM
FILE[ui]=$UI_BUNDLE
FILE[web]=$WEB_BUNDLE
for k in "${KINDS[@]}"; do
  f=${FILE[$k]}
  [ -s "$f" ] || die "no $k bundle at $f (tools/build_ui_bundle.sh and tools/build_web.sh build them)"
  if [ -n "$UNSQUASHFS" ]; then
    list=$("$UNSQUASHFS" -l "$f" 2>/dev/null) || die "$f is not a SquashFS image"
    case $k in
    ui) need="bin/nspanel-app share/ui/fonts" ;;
    web) need="usr/bin/panelos-web usr/lib/libgbm.so.1" ;;
    esac
    for p in $need; do
      grep -qx "squashfs-root/$p" <<<"$list" || die "$f is not the $k bundle (no $p in it)"
    done
  else
    echo "note: unsquashfs not found, the contents of $f are not checked"
  fi
  SIZE[$k]=$(stat -c %s "$f")
  SUM[$k]=$(md5sum < "$f" | cut -d' ' -f1)
done

########## the panel now ##########
TARGETS="/mnt/data/panelos-ui.squashfs /mnt/persist/panelos-ui.squashfs /mnt/persist/panelos-web.squashfs"
probe=$(push "sizes='${SIZE[*]}' files='$TARGETS' sh -s" <<'PROBE'
freekb() { df -Pk "$1" 2>/dev/null | awk 'NR==2 {print $4}'; }
if mountpoint -q /mnt/persist; then echo "persist 1 $(freekb /mnt/persist)"; else echo "persist 0 0"; fi
if mountpoint -q /mnt/data; then echo "data 1 $(freekb /mnt/data)"; else echo "data 0 0"; fi
for s in S50panelos-web S51panelos-ui S90nspanel-app; do [ -x /etc/init.d/$s ] && echo "init $s"; done
[ -x /mnt/data/panelos-ui-dev/bin/nspanel-app ] && echo "uidev 1"
for t in $files; do
	for f in "$t" "$t.new" "/mnt/data/${t##*/}.new"; do
		[ -f "$f" ] || continue
		s=$(wc -c < "$f" | tr -d ' ')
		m=-
		# md5 only where it can match a new bundle
		case " $sizes " in *" $s "*) m=$(md5sum < "$f" | cut -d' ' -f1) ;; esac
		echo "file $f $s $m"
	done
done
echo end
PROBE
) || die "cannot reach the panel over SSH ($HOST)"
grep -qx end <<<"$probe" || die "could not read the panel's state ($HOST)"

PERSIST=0 DATA=0 UIDEV=0 INIT=""
declare -A FREE HAVE_SIZE HAVE_SUM
while read -r a b c d; do
  case $a in
  persist) PERSIST=$b; FREE[persist]=$c ;;
  data) DATA=$b; FREE[data]=$c ;;
  init) INIT="$INIT $b" ;;
  uidev) UIDEV=1 ;;
  file) HAVE_SIZE[$b]=$c; HAVE_SUM[$b]=$d ;;
  esac
done <<<"$probe"
[ "$PERSIST" = 1 ] || die "the panel's persistent partition (/mnt/persist) is not mounted"
if [ "$DATA" != 1 ]; then
  # Without the data partition /mnt/data is a folder in RAM.
  for f in "${!HAVE_SIZE[@]}"; do
    case $f in /mnt/data/*) unset "HAVE_SIZE[$f]" "HAVE_SUM[$f]" ;; esac
  done
fi
need_init() { [[ " $INIT " == *" $1 "* ]] || die "the panel's PanelOS is too old for the bundles (no /etc/init.d/$1): update it first (docs/install.md, Updating later)"; }
need_init S90nspanel-app
[ "$WANT_UI" = 1 ] && need_init S51panelos-ui
[ "$WANT_WEB" = 1 ] && need_init S50panelos-web

########## the plan ##########
fs_of() { case $1 in /mnt/data/*) echo data ;; *) echo persist ;; esac; }
declare -A TARGET STAGED MODE EXTRA
TODO=()
for k in "${KINDS[@]}"; do
  if [ "$k" = web ]; then
    t=/mnt/persist/panelos-web.squashfs
  elif [ "$DATA" = 1 ]; then
    t=/mnt/data/panelos-ui.squashfs
    # an older copy on the persistent partition is not used any more
    [ -n "${HAVE_SIZE[/mnt/persist/panelos-ui.squashfs]:-}" ] && EXTRA[$k]=/mnt/persist/panelos-ui.squashfs
  else
    t=/mnt/persist/panelos-ui.squashfs
  fi
  TARGET[$k]=$t
  if [ "${HAVE_SUM[$t]:-}" = "${SUM[$k]}" ]; then
    echo "$k bundle: the same one is on the panel already ($t)"
    continue
  fi
  TODO+=("$k")
  need=$(( $(kb "${SIZE[$k]}") + MARGIN ))
  tfs=$(fs_of "$t")
  old=$(kb "${HAVE_SIZE[$t]:-0}")
  # A left-over upload under the temporary name is overwritten (or used, when it is complete).
  beside=$(( FREE[$tfs] + $(kb "${HAVE_SIZE[$t.new]:-0}") ))
  via=$(( FREE[data] + $(kb "${HAVE_SIZE[/mnt/data/${t##*/}.new]:-0}") ))
  if [ "$beside" -ge "$need" ]; then
    MODE[$k]=beside
    STAGED[$k]=$t.new
    FREE[$tfs]=$(( beside - need ))
  elif [ "$tfs" = persist ] && [ "$DATA" = 1 ] && [ "$via" -ge "$need" ] && [ $(( beside + old )) -ge "$need" ]; then
    MODE[$k]=via
    STAGED[$k]=/mnt/data/${t##*/}.new
    FREE[data]=$(( via - need ))
    FREE[persist]=$(( beside + old - need ))
  elif [ $(( beside + old )) -ge "$need" ]; then
    MODE[$k]=replace
    STAGED[$k]=$t.new
    FREE[$tfs]=$(( beside + old - need ))
  else
    die "not enough room on the panel for the $k bundle: $(( (need - beside - old + 1023) / 1024 )) MB more needed on $(dirname "$t")"
  fi
done
[ ${#TODO[@]} -gt 0 ] || exit 0

echo "panel $HOST:"
ask=0
for k in "${TODO[@]}"; do
  case ${MODE[$k]} in
  beside) how="uploaded under a temporary name, then renamed into place" ;;
  via) how="uploaded to /mnt/data, then copied into place (no room for two copies in $(dirname "${TARGET[$k]}"))" ;;
  replace) how="no room for two copies: the old one is removed before the upload"; ask=1 ;;
  esac
  echo "  $k bundle, $(mb "${SIZE[$k]}") MB -> ${TARGET[$k]}: $how"
done
if [ "$WANT_UI" = 1 ] && [ "$UIDEV" = 1 ]; then
  echo "  note: this panel runs the interface from the development folder /mnt/data/panelos-ui-dev;"
  echo "        the UI bundle is used only once that folder is gone"
fi
[ "${DRY_RUN:-}" = 1 ] && { echo "DRY_RUN=1: nothing changed"; exit 0; }
if [ "$ask" = 1 ] && [ "${YES:-}" != 1 ]; then
  echo "Until the new bundle is in place, the panel shows the image's simple interface."
  read -r -p "Type yes to go on: " answer </dev/tty
  [ "$answer" = yes ] || die "stopped, nothing changed"
fi

########## the panel's part ##########
RUN="$(date +%Y%m%d-%H%M%S)-$$"
SCRIPT=/tmp/panelos-bundles-$RUN.sh
items() { # mode... -> lines "kind staged target md5 size extra"
  local k m
  for k in "${TODO[@]}"; do
    for m in "$@"; do
      [ "${MODE[$k]}" = "$m" ] && echo "$k ${STAGED[$k]} ${TARGET[$k]} ${SUM[$k]} ${SIZE[$k]} ${EXTRA[$k]:--}"
    done
  done
  return 0
}
panel_script() {
  echo "ITEMS='$(items beside via replace)'"
  echo "CLEAR='$(items replace)'"
  cat <<'PANEL'
# PanelOS bundle install, the panel's part (written by tools/install_bundles.sh).
# Each line of ITEMS / CLEAR: kind staged target md5 size extra (an old copy to remove, or -).
ACTION=$1
LOG=$2
exec 9> /tmp/panelos-bundles.lock
flock -n 9 || exit 0
say() { echo "$*" >> "$LOG"; }
say "start $ACTION"
md5() { md5sum < "$1" | cut -d' ' -f1; }
bytes() { wc -c < "$1" | tr -d ' '; }
ENVFILE=/tmp/panelos-bundles.env

app_stop() {
	: > "$ENVFILE"
	pid=$(pidof nspanel-app | cut -d' ' -f1)
	# The app starts again with the development switches it ran with.
	[ -n "$pid" ] && tr '\000' '\n' < "/proc/$pid/environ" | grep -E '^(NSPANEL|PANELOS|WEBKIT)_[A-Za-z0-9_]*=' |
		sed "s/'/'\\\\''/g; s/=/='/; s/\$/'/; s/^/export /" > "$ENVFILE"
	/etc/init.d/S90nspanel-app stop > /dev/null 2>&1
	n=0
	while pidof nspanel-app > /dev/null && [ $n -lt 10 ]; do
		sleep 1
		n=$((n + 1))
	done
	for p in $(pidof nspanel-app); do kill -9 "$p"; done
	# The dashboard browser and its helpers run chrooted in the web bundle.
	for p in /proc/[0-9]*; do
		[ "$(readlink "$p/root" 2> /dev/null)" = /opt/panelos-web ] && kill -9 "${p#/proc/}" 2> /dev/null
	done
	sleep 1
}

app_start() {
	rm -f /tmp/nspanel-gpu-off
	# 9>&-: the app must not hold this script's lock.
	(. "$ENVFILE"; /etc/init.d/S90nspanel-app start > /dev/null 2>&1 9>&-)
	sleep 3
	pid=$(pidof nspanel-app | cut -d' ' -f1)
	say "app: ${pid:+$(readlink "/proc/$pid/exe")}"
}

root_of() { case $1 in ui) echo /opt/panelos-ui ;; web) echo /opt/panelos-web ;; esac; }
init_of() { case $1 in ui) echo /etc/init.d/S51panelos-ui ;; web) echo /etc/init.d/S50panelos-web ;; esac; }
unmount() {
	"$(init_of "$1")" stop > /dev/null 2>&1
	if mountpoint -q "$(root_of "$1")"; then
		say "$1: still in use by $(fuser -m "$(root_of "$1")" 2> /dev/null)"
		return 1
	fi
}
mount_again() { "$(init_of "$1")" start > /dev/null 2>&1 9>&-; }

check_uploads() {
	while read -r kind staged target sum size extra; do
		[ -n "$kind" ] || continue
		if [ ! -f "$staged" ] || [ "$(bytes "$staged")" != "$size" ] || [ "$(md5 "$staged")" != "$sum" ]; then
			say "$kind: the upload $staged is missing or not complete"
			return 1
		fi
	done <<- EOF
		$ITEMS
	EOF
}

swap() {
	check_uploads || { say "result: failed, nothing changed"; return; }
	app_stop
	status=ok
	while read -r kind staged target sum size extra; do
		[ -n "$kind" ] || continue
		if ! unmount "$kind"; then
			if [ "${staged%/*}" = "${target%/*}" ]; then
				mv -f "$staged" "$target"
				sync
				say "$kind: in place, used after the next restart of the panel"
				[ "$status" = failed ] || status=restart
			else
				say "$kind: nothing changed (the upload stays in $staged)"
				status=failed
			fi
			# what the stop took away (the web bundle's bound folders) comes back
			mount_again "$kind"
			continue
		fi
		if [ "${staged%/*}" = "${target%/*}" ]; then
			mv -f "$staged" "$target"
		else
			rm -f "$target" "$target.new"
			sync
			if cp "$staged" "$target.new" && sync && [ "$(md5 "$target.new")" = "$sum" ]; then
				mv -f "$target.new" "$target"
				rm -f "$staged"
			else
				rm -f "$target.new"
				say "$kind: could not be copied to $target (the upload stays in $staged)"
				status=failed
			fi
		fi
		[ "$extra" = - ] || rm -f "$extra"
		sync
		mount_again "$kind"
		if mountpoint -q "$(root_of "$kind")"; then
			[ -f "$target" ] && say "$kind: installed"
		else
			say "$kind: not mounted"
			status=failed
		fi
	done <<- EOF
		$ITEMS
	EOF
	app_start
	say "result: $status"
}

clear_old() {
	app_stop
	status=ok
	while read -r kind staged target sum size extra; do
		[ -n "$kind" ] || continue
		if unmount "$kind"; then
			rm -f "$target"
			[ "$extra" = - ] || rm -f "$extra"
			say "$kind: old bundle removed"
		else
			status=failed
		fi
		mount_again "$kind"
	done <<- EOF
		$CLEAR
	EOF
	sync
	app_start
	say "result: $status"
}

case $ACTION in
swap) swap ;;
clear) clear_old ;;
esac
PANEL
}

# Starts a step of the panel's part detached and waits for its result line.
run_on_panel() {
  local action=$1 log=/tmp/panelos-bundles-$RUN-$1.log out="" try
  # A retry after a lost answer finds the log and does not start the step twice.
  for try in 1 2 3 4 5; do
    if push "[ -e $log ] || { cat > $SCRIPT && (nohup sh $SCRIPT $action $log < /dev/null > /dev/null 2>&1 &); }; echo started" < "$LOCAL_SCRIPT" 2>/dev/null | grep -q started; then
      break
    fi
    [ "$try" = 5 ] && die "cannot reach the panel to start the $action step"
    sleep 3
  done
  for try in $(seq 1 90); do
    sleep 4
    out=$(on_panel "cat $log" 2>/dev/null) || continue
    grep -q '^result:' <<<"$out" && break
  done
  sed 's/^/  panel: /' <<<"$out"
  grep -q '^result:' <<<"$out" || die "the panel did not finish the $action step in time; its log: $log on the panel"
  RESULT=$(sed -n 's/^result: //p' <<<"$out")
}

upload() {
  local k=$1 staged=${STAGED[$1]} got try
  got=$(on_panel "[ -f $staged ] && [ \"\$(wc -c < $staged | tr -d ' ')\" = ${SIZE[$k]} ] && md5sum < $staged | cut -d' ' -f1" 2>/dev/null || true)
  if [ "$got" = "${SUM[$k]}" ]; then
    echo "$k bundle: uploaded already ($staged)"
    return 0
  fi
  for try in 1 2 3; do
    echo "$k bundle: uploading $(mb "${SIZE[$k]}") MB to $staged"
    if dd if="${FILE[$k]}" bs=1M status=progress | timeout 1800 ssh "${SSH_OPTS[@]}" "root@$HOST" "cat > $staged"; then
      got=$(on_panel "[ \"\$(wc -c < $staged | tr -d ' ')\" = ${SIZE[$k]} ] && md5sum < $staged | cut -d' ' -f1" 2>/dev/null || true)
      [ "$got" = "${SUM[$k]}" ] && { echo "$k bundle: uploaded and checked"; return 0; }
    fi
    echo "$k bundle: the upload is not complete, once more"
    sleep 3
  done
  on_panel "rm -f $staged" 2>/dev/null || true
  return 1
}

LOCAL_SCRIPT=$(mktemp)
trap 'rm -f "$LOCAL_SCRIPT"' EXIT
panel_script > "$LOCAL_SCRIPT"

# First the uploads that need nothing removed: if one fails, nothing has changed.
for k in "${TODO[@]}"; do
  [ "${MODE[$k]}" = replace ] && continue
  upload "$k" || die "the $k bundle could not be uploaded; nothing changed on the panel"
done
if [ -n "$(items replace)" ]; then
  echo "removing the old bundle(s) to make room"
  run_on_panel clear
  [ "$RESULT" = ok ] || die "the old bundle is still in use: restart the panel and run this again"
  for k in "${TODO[@]}"; do
    [ "${MODE[$k]}" = replace ] || continue
    upload "$k" || die "the $k bundle could not be uploaded; the old one is removed, so run this again (the panel shows the image's interface until then)"
  done
fi
echo "putting the bundle(s) in place"
run_on_panel swap
case $RESULT in
ok) ;;
restart) echo "restart the panel to use the new bundle (the reboot command of docs/install.md, Updating later)" ;;
*) die "not installed: restart the panel and run this again (completed uploads are not repeated)" ;;
esac
on_panel "rm -f $SCRIPT /tmp/panelos-bundles-$RUN-*.log /tmp/panelos-bundles.env" 2>/dev/null || true

# The app from the bundle switches itself off when it ends within 30 s three times.
sleep 25
state=$(on_panel 'pid=$(pidof nspanel-app | cut -d" " -f1); echo "exe ${pid:+$(readlink /proc/$pid/exe)}"
  [ -e /tmp/nspanel-gpu-off ] && echo gpu-off
  mountpoint -q /opt/panelos-ui && echo ui
  [ -x /mnt/data/panelos-ui-dev/bin/nspanel-app ] && echo uidev
  [ -e /opt/panelos-web/usr/lib/libgbm.so.1 ] && echo web; true' 2>/dev/null || true)
case $(sed -n 's/^exe //p' <<<"$state") in
/opt/panelos-ui/bin/nspanel-app)
  if grep -qx uidev <<<"$state"; then
    echo "the app runs from the development folder /mnt/data/panelos-ui-dev, on the GPU"
  else
    echo "the app runs from the UI bundle, on the GPU"
  fi
  ;;
/usr/bin/nspanel-app)
  echo "the image's app runs (software renderer):"
  if grep -qx gpu-off <<<"$state"; then
    echo "  the bundle's app ended within 30 s three times (until the next restart; its messages: /var/log/messages on the panel)"
  elif ! grep -qx ui <<<"$state"; then
    echo "  no UI bundle is mounted"
  elif ! grep -qx web <<<"$state"; then
    echo "  the bundle's app needs the web bundle too (its GPU libraries)"
  fi
  ;;
*) echo "the app is not running right now (it is restarted by itself; check again in a minute)" ;;
esac
grep -qx web <<<"$state" && echo "dashboard mode: the web bundle is mounted"
echo "done"
