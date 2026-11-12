#!/bin/sh
# Restart avahi-daemon on DHCP renew/bound to fix stale mDNS
case "$1" in
    renew|bound)
        avahi-daemon --kill 2>/dev/null
        avahi-daemon -D 2>/dev/null
        ;;
esac
