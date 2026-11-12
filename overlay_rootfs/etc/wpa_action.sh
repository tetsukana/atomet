#!/bin/sh
#
# wpa_cli action script for wlan0.
# Invoked by: wpa_cli -i wlan0 -a /etc/wpa_action.sh -B
# Args: $1 = interface, $2 = event (CONNECTED / DISCONNECTED / ...)
#
# WiFi が (再)接続するたびに DHCP リースを取り直す。これがないと
# wpa_supplicant が再アソシエートしても IP アドレスが付かないままになる。
#
IFACE="$1"
EVENT="$2"
PIDFILE="/var/run/udhcpc.${IFACE}.pid"

case "$EVENT" in
    CONNECTED)
        # 既存の udhcpc を止めてから取り直す (リース重複防止)
        [ -f "$PIDFILE" ] && kill "$(cat "$PIDFILE")" 2>/dev/null
        udhcpc -i "$IFACE" -x hostname:atomet -p "$PIDFILE" -b
        ;;
    DISCONNECTED)
        # wpa_supplicant が自動で再接続を試みる。リースはそのまま保持し、
        # 再接続時の CONNECTED イベントで取り直す。
        ;;
esac

exit 0
