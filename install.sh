#!/usr/bin/env bash
# Build ambientd and install: binary, systemd user unit, suspend-recovery unit.
set -euo pipefail

msg()  { printf '\033[1;32m==>\033[0m %s\n' "$*"; }
warn() { printf '\033[1;33m==> WARNING:\033[0m %s\n' "$*"; }
cd "$(dirname "$0")"

msg "Building release binary"
cargo build --release

msg "Installing binary to ~/.local/bin"
mkdir -p ~/.local/bin
install -m755 target/release/ambientd ~/.local/bin/ambientd

msg "Installing systemd user unit"
mkdir -p ~/.config/systemd/user
systemctl --user stop ambientd.service 2>/dev/null || true   # clears any transient instance
cp ambientd.user.service ~/.config/systemd/user/ambientd.service
systemctl --user daemon-reload
systemctl --user enable --now ambientd.service

msg "Installing suspend-recovery unit (needs sudo)"
if sudo cp als-reload.service /etc/systemd/system/ &&
   sudo systemctl daemon-reload &&
   sudo systemctl enable --now als-reload.service; then
    :
else
    warn "Could not install als-reload.service (no sudo?). Without it the ALS"
    warn "dies after suspend. Fix later by re-running this script in a terminal,"
    warn "or manually:"
    warn "  sudo cp als-reload.service /etc/systemd/system/"
    warn "  sudo systemctl enable --now als-reload.service"
fi

sleep 2
msg "Status:"
systemctl --user is-active ambientd && journalctl --user -u ambientd -n 2 --no-pager -o cat || true
msg "Done. Follow readings with: journalctl --user -u ambientd -f"
