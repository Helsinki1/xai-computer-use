#!/usr/bin/env bash
# Installs the Linux computer-use daemon and MCP relay to their fixed paths.
# Usage: scripts/install.sh [--build]
set -euo pipefail

subtree="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
install_dir="${HOME}/.local/libexec/grok-computer-use"
daemon="grok-computer-use-daemon"
relay="grok-computer-use-mcp"

if [[ "${1:-}" == "--build" ]]; then
    (cd "${subtree}" && cargo build --release)
fi

for binary in "${daemon}" "${relay}"; do
    if [[ ! -x "${subtree}/target/release/${binary}" ]]; then
        echo "error: ${subtree}/target/release/${binary} is missing; run with --build first" >&2
        exit 1
    fi
done

if [[ "${XDG_SESSION_TYPE:-}" != "x11" ]]; then
    echo "warning: XDG_SESSION_TYPE is '${XDG_SESSION_TYPE:-unset}'; the MVP supports X11 sessions only" >&2
fi

mkdir -p "${install_dir}"
chmod 0700 "${install_dir}"
install -m 0755 "${subtree}/target/release/${daemon}" "${install_dir}/${daemon}.new"
install -m 0755 "${subtree}/target/release/${relay}" "${install_dir}/${relay}.new"
mv -f "${install_dir}/${daemon}.new" "${install_dir}/${daemon}"
mv -f "${install_dir}/${relay}.new" "${install_dir}/${relay}"

echo "installed: ${install_dir}/${daemon}"
echo "installed: ${install_dir}/${relay}"

unit_dir="${HOME}/.config/systemd/user"
if command -v systemctl >/dev/null 2>&1; then
    mkdir -p "${unit_dir}"
    cat > "${unit_dir}/grok-computer-use.service" <<UNIT
[Unit]
Description=Grok Computer Use daemon (X11)

[Service]
ExecStart=${install_dir}/${daemon}
Restart=on-failure

[Install]
WantedBy=default.target
UNIT
    echo "systemd user unit written: ${unit_dir}/grok-computer-use.service"
    echo "enable it with: systemctl --user enable --now grok-computer-use"
else
    echo "systemd not found; start the daemon manually: ${install_dir}/${daemon}"
fi
