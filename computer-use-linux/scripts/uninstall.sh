#!/usr/bin/env bash
# Removes the installed binaries and the systemd user unit. Durable receipts
# in ~/.local/share/grok-computer-use are intentionally preserved: deleting
# them would break the no-retry guarantee for old action identifiers.
set -euo pipefail

install_dir="${HOME}/.local/libexec/grok-computer-use"
unit="${HOME}/.config/systemd/user/grok-computer-use.service"

if command -v systemctl >/dev/null 2>&1; then
    systemctl --user disable --now grok-computer-use 2>/dev/null || true
fi
rm -f "${unit}"
rm -f "${install_dir}/grok-computer-use-daemon" "${install_dir}/grok-computer-use-mcp"
rmdir "${install_dir}" 2>/dev/null || true
echo "uninstalled; receipts in ~/.local/share/grok-computer-use were preserved"
