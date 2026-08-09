#!/usr/bin/env bash
set -euo pipefail

DISPLAY_NUM="${DISPLAY_NUM:-:1}"
SCREEN_GEOMETRY="${SCREEN_GEOMETRY:-1440x900x24}"
RUNTIME_ROOT="${XDG_RUNTIME_DIR:-/tmp/grok-computer-use-runtime}"
LOG_ROOT="${LOG_ROOT:-/tmp/grok-computer-use-logs}"
HOME="${HOME:-/root}"
INSTALL_DIR="${HOME}/.local/libexec/grok-computer-use"

mkdir -p "${RUNTIME_ROOT}" "${LOG_ROOT}" "${HOME}/.config/openbox"
chmod 0700 "${RUNTIME_ROOT}"

export DISPLAY="${DISPLAY_NUM}"
export XDG_SESSION_TYPE="x11"
export XDG_RUNTIME_DIR="${RUNTIME_ROOT}"
export LIBGL_ALWAYS_SOFTWARE=1

find_novnc_proxy() {
    if command -v novnc_proxy >/dev/null 2>&1; then
        command -v novnc_proxy
        return 0
    fi
    if [[ -x /usr/share/novnc/utils/novnc_proxy ]]; then
        printf '%s\n' /usr/share/novnc/utils/novnc_proxy
        return 0
    fi
    return 1
}

require_binary() {
    if ! command -v "$1" >/dev/null 2>&1; then
        printf 'missing required binary: %s\n' "$1" >&2
        exit 1
    fi
}

require_binary Xvfb
require_binary chromium
require_binary openbox
require_binary xterm
require_binary x11vnc
require_binary tail

NOVNC_PROXY="$(find_novnc_proxy)" || {
    printf 'noVNC proxy was not found\n' >&2
    exit 1
}

cat > "${HOME}/.config/openbox/autostart" <<'EOF'
xsetroot -solid "#1e2329" &
xterm \
  -fa Monospace \
  -fs 12 \
  -geometry 120x30+40+40 \
  -title "linux-computer-use-mvp" \
  -e bash -lc 'printf "linux-computer-use-mvp\n\nThis X11 desktop is running inside Modal.\nThe daemon logs are in /tmp/grok-computer-use-logs.\n\n"; exec bash' &
xterm \
  -fa Monospace \
  -fs 12 \
  -geometry 120x30+220+80 \
  -title "grok-build" \
  -e bash -lc '${HOME}/computer-use-linux/scripts/start_modal_grok.sh' &
EOF

Xvfb "${DISPLAY}" -screen 0 "${SCREEN_GEOMETRY}" -ac +extension RANDR >"${LOG_ROOT}/xvfb.log" 2>&1 &
XVFB_PID=$!

cleanup() {
    kill "${XVFB_PID}" >/dev/null 2>&1 || true
}
trap cleanup EXIT

for _ in $(seq 1 30); do
    if xdpyinfo -display "${DISPLAY}" >/dev/null 2>&1; then
        break
    fi
    sleep 1
done
xdpyinfo -display "${DISPLAY}" >/dev/null 2>&1 || {
    printf 'Xvfb did not become ready\n' >&2
    exit 1
}

openbox >"${LOG_ROOT}/openbox.log" 2>&1 &
sleep 2

chromium \
  --no-sandbox \
  --disable-dev-shm-usage \
  --disable-gpu \
  --start-maximized \
  --new-window \
  "about:blank" \
  >"${LOG_ROOT}/browser.log" 2>&1 &

touch "${LOG_ROOT}/daemon.log"
"${INSTALL_DIR}/grok-computer-use-daemon" >"${LOG_ROOT}/daemon.log" 2>&1 &
DAEMON_PID=$!
for _ in $(seq 1 30); do
    if [[ -S "${XDG_RUNTIME_DIR}/grok-computer-use/agent-v2.sock" ]]; then
        break
    fi
    if ! kill -0 "${DAEMON_PID}" >/dev/null 2>&1; then
        printf 'daemon exited before socket became ready\n' >&2
        tail -n 80 "${LOG_ROOT}/daemon.log" >&2 || true
        exit 1
    fi
    sleep 1
done
[[ -S "${XDG_RUNTIME_DIR}/grok-computer-use/agent-v2.sock" ]] || {
    printf 'daemon socket did not become ready\n' >&2
    if ! kill -0 "${DAEMON_PID}" >/dev/null 2>&1; then
        tail -n 80 "${LOG_ROOT}/daemon.log" >&2 || true
    fi
    exit 1
}

if [[ "${RUN_SCRIPTED_DEMO:-0}" == "1" ]]; then
  python3 "${HOME}/computer-use-linux/scripts/modal_restaurant_demo.py" >"${LOG_ROOT}/demo.log" 2>&1 &
fi

x11vnc \
  -display "${DISPLAY}" \
  -rfbport 5900 \
  -forever \
  -shared \
  -nopw \
  -xkb \
  >"${LOG_ROOT}/x11vnc.log" 2>&1 &

exec "${NOVNC_PROXY}" --vnc localhost:5900 --listen 6080
