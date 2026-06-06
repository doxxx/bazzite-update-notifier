#!/usr/bin/env bash
#
# uninstall.sh — remove bazzite-update-notifier from the current user's
# session. Stops and disables the systemd --user unit, then removes the
# binary, unit file, XDG autostart entry, and icon.

set -euo pipefail

if [[ -n "${XDG_CONFIG_HOME:-}" ]]; then
    CONFIG_DIR="${XDG_CONFIG_HOME}"
else
    CONFIG_DIR="${HOME}/.config"
fi

if [[ -n "${XDG_DATA_HOME:-}" ]]; then
    DATA_DIR="${XDG_DATA_HOME}"
else
    DATA_DIR="${HOME}/.local/share"
fi

BIN_DIR="${HOME}/.local/bin"
SYSTEMD_USER_DIR="${CONFIG_DIR}/systemd/user"
AUTOSTART_DIR="${CONFIG_DIR}/autostart"
ICON_DIR="${DATA_DIR}/icons/hicolor/64x64/apps"

if command -v systemctl >/dev/null 2>&1; then
    if systemctl --user is-enabled --quiet bazzite-update-notifier.service 2>/dev/null; then
        echo "Stopping and disabling bazzite-update-notifier.service"
        systemctl --user disable --now bazzite-update-notifier.service
    fi
    systemctl --user daemon-reload
fi

echo "Removing binary"
rm -f "${BIN_DIR}/bazzite-update-notifier"

echo "Removing systemd --user unit"
rm -f "${SYSTEMD_USER_DIR}/bazzite-update-notifier.service"

echo "Removing XDG autostart entry"
rm -f "${AUTOSTART_DIR}/bazzite-update-notifier.desktop"

echo "Removing icon"
rm -f "${ICON_DIR}/bazzite-update-notifier.png"

echo
echo "Uninstall complete."
