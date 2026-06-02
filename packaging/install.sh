#!/usr/bin/env bash
#
# install.sh — install bazzite-update-notifier into the current user's
# session. Copies the binary, the systemd --user unit, and the XDG
# autostart .desktop, then enables and starts the service.
#
# Re-run this script after rebuilding the binary to update the install.

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
CACHE_DIR="${XDG_CACHE_HOME:-${HOME}/.cache}/bazzite-update-notifier"

# Resolve where this script lives so it works whether invoked by absolute
# or relative path.
SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" &>/dev/null && pwd)"

# Support two layouts:
#   release bundle — binary is co-located with this script in the same directory
#   source checkout — binary is at <repo-root>/target/release/
if [[ -x "${SCRIPT_DIR}/bazzite-update-notifier" ]]; then
    BIN_SRC="${SCRIPT_DIR}/bazzite-update-notifier"
    ICON_SRC="${SCRIPT_DIR}/icon-update-available.png"
else
    REPO_ROOT="$(cd -- "${SCRIPT_DIR}/.." &>/dev/null && pwd)"
    BIN_SRC="${REPO_ROOT}/target/release/bazzite-update-notifier"
    ICON_SRC="${REPO_ROOT}/assets/icon-update-available.png"
fi

if [[ ! -x "${BIN_SRC}" ]]; then
    echo "error: ${BIN_SRC} not found or not executable." >&2
    echo "       Run \`mise run build\` (or \`cargo build --release\`) first." >&2
    exit 1
fi

mkdir -p "${BIN_DIR}" "${SYSTEMD_USER_DIR}" "${AUTOSTART_DIR}" "${ICON_DIR}" "${CACHE_DIR}"

echo "Installing binary -> ${BIN_DIR}/bazzite-update-notifier"
install -m 0755 "${BIN_SRC}" "${BIN_DIR}/bazzite-update-notifier"

echo "Installing systemd --user unit -> ${SYSTEMD_USER_DIR}"
install -m 0644 "${SCRIPT_DIR}/bazzite-update-notifier.service" \
    "${SYSTEMD_USER_DIR}/bazzite-update-notifier.service"

echo "Installing XDG autostart entry -> ${AUTOSTART_DIR}"
install -m 0644 "${SCRIPT_DIR}/bazzite-update-notifier.desktop" \
    "${AUTOSTART_DIR}/bazzite-update-notifier.desktop"

echo "Installing icon -> ${ICON_DIR}"
install -m 0644 "${ICON_SRC}" \
    "${ICON_DIR}/bazzite-update-notifier.png"

# Reload and enable. We use --now so the daemon starts immediately;
# subsequent logins use the autostart entry.
if command -v systemctl >/dev/null 2>&1; then
    echo "Reloading user systemd"
    systemctl --user daemon-reload
    echo "Enabling and starting bazzite-update-notifier.service"
    systemctl --user enable --now bazzite-update-notifier.service
else
    echo "warning: systemctl not found; service files installed but not enabled" >&2
fi

echo
echo "Installation complete. Status:"
systemctl --user --no-pager status bazzite-update-notifier.service || true
