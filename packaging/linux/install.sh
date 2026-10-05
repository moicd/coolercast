#!/bin/sh
# Installs or removes CoolerCast on a Linux system with systemd.
#
#   sudo ./install.sh              install (or upgrade) and start the service
#   sudo ./install.sh --uninstall  stop and remove it; settings in /etc/coolercast are kept
#
# The script copies itself to /usr/local/share/coolercast so it can uninstall later.

set -eu

BIN=/usr/local/bin/coolercast
UNIT=/etc/systemd/system/coolercast.service
RULES=/etc/udev/rules.d/99-coolercast.rules
SHARE=/usr/local/share/coolercast
HERE=$(cd "$(dirname "$0")" && pwd)

if [ "$(id -u)" -ne 0 ]; then
    echo "Run this script as root: sudo $0 ${1:-}" >&2
    exit 1
fi
if ! command -v systemctl >/dev/null 2>&1; then
    echo "systemd is required. Without it, run '$HERE/coolercast run' as root from your init system." >&2
    exit 1
fi

reload_udev() {
    udevadm control --reload-rules 2>/dev/null || true
    udevadm trigger --subsystem-match=hidraw 2>/dev/null || true
}

case "${1:-}" in
    "")
        install -Dm755 "$HERE/coolercast" "$BIN"
        install -Dm644 "$HERE/coolercast.service" "$UNIT"
        install -Dm644 "$HERE/99-coolercast.rules" "$RULES"
        if [ "$HERE" != "$SHARE" ]; then
            install -Dm755 "$HERE/install.sh" "$SHARE/install.sh"
        fi
        reload_udev
        systemctl daemon-reload
        systemctl enable coolercast.service
        systemctl restart coolercast.service
        echo "CoolerCast is installed and running."
        echo "Check it with 'coolercast status'; change settings with 'coolercast set key=value'."
        echo "To uninstall: sudo $SHARE/install.sh --uninstall"
        ;;
    --uninstall)
        systemctl disable --now coolercast.service 2>/dev/null || true
        rm -f "$UNIT" "$RULES" "$BIN"
        rm -rf "$SHARE"
        systemctl daemon-reload
        reload_udev
        echo "CoolerCast was removed. Settings were kept in /etc/coolercast."
        ;;
    *)
        echo "usage: sudo $0 [--uninstall]" >&2
        exit 2
        ;;
esac
