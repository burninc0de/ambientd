#!/usr/bin/env bash
# Remove everything ambientd installed.
set -euo pipefail

msg()  { printf '\033[1;32m==>\033[0m %s\n' "$*"; }
warn() { printf '\033[1;33m==> WARNING:\033[0m %s\n' "$*"; }

msg "Stopping and disabling user unit"
systemctl --user disable --now ambientd.service 2>/dev/null || true
rm -f ~/.config/systemd/user/ambientd.service
systemctl --user daemon-reload
rm -f /run/user/"$(id -u)"/ambientd.lock

msg "Removing binary"
rm -f ~/.local/bin/ambientd

msg "Removing suspend-recovery unit (needs sudo)"
if sudo -n true 2>/dev/null || sudo -v; then
    sudo systemctl disable --now als-reload.service 2>/dev/null || true
    sudo rm -f /etc/systemd/system/als-reload.service
    sudo systemctl daemon-reload
else
    warn "Could not get sudo. Run these manually:"
    warn "  sudo systemctl disable --now als-reload.service"
    warn "  sudo rm -f /etc/systemd/system/als-reload.service"
fi

msg "Uninstalled."
