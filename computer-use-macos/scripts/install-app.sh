#!/usr/bin/env bash

set -euo pipefail

component_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
source_app="${component_root}/dist/Grok Computer Use.app"
applications_dir="${HOME}/Applications"
target_app="${applications_dir}/Grok Computer Use.app"
local_development="${GROK_COMPUTER_USE_LOCAL_DEV:-0}"
agent_socket="${HOME}/Library/Application Support/com.xai.grok.computer-use/agent-v2.sock"
socket_identity_before=""

app_is_running() {
  /usr/bin/pgrep -u "$(/usr/bin/id -u)" -x GrokComputerUseApp >/dev/null
}

agent_is_ready() {
  app_is_running || return 1
  [[ -S "${agent_socket}" ]] || return 1
  local socket_identity_now
  socket_identity_now="$(/usr/bin/stat -f '%d:%i' "${agent_socket}" 2>/dev/null)" || return 1
  [[ -z "${socket_identity_before}" || "${socket_identity_now}" != "${socket_identity_before}" ]]
}

terminate_running_app() {
  if ! app_is_running; then
    return 0
  fi
  /usr/bin/pkill -TERM -u "$(/usr/bin/id -u)" -x GrokComputerUseApp || return 1
  for _ in {1..50}; do
    if ! app_is_running; then
      return 0
    fi
    /bin/sleep 0.1
  done
  return 1
}

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "install-app.sh requires macOS" >&2
  exit 1
fi
if [[ "$(uname -m)" != "arm64" && "${local_development}" != "1" ]]; then
  echo "Intel installation requires GROK_COMPUTER_USE_LOCAL_DEV=1" >&2
  exit 1
fi
if [[ -S "${agent_socket}" ]]; then
  socket_identity_before="$(/usr/bin/stat -f '%d:%i' "${agent_socket}" 2>/dev/null || true)"
fi
if [[ "${1:-}" == "--build" || ! -d "${source_app}" ]]; then
  if [[ "${local_development}" == "1" ]]; then
    "${component_root}/scripts/build-app.sh" debug
  else
    "${component_root}/scripts/build-app.sh" release
  fi
fi
if [[ -L "${applications_dir}" || -L "${target_app}" ]]; then
  echo "refusing to install through a symbolic link" >&2
  exit 1
fi
verify_script="${component_root}/scripts/verify-bundle.sh"
if [[ "${local_development}" == "1" ]]; then
  verify_script="${component_root}/scripts/verify-local-bundle.sh"
fi
"${verify_script}" "${source_app}"
/bin/mkdir -p "${applications_dir}"

staging_root="$(/usr/bin/mktemp -d "${applications_dir}/.grok-computer-use.install.XXXXXX")"
staged_app="${staging_root}/Grok Computer Use.app"
cleanup() {
  if [[ -d "${staging_root}" ]]; then
    /bin/rm -rf "${staging_root}"
  fi
}
trap cleanup EXIT
/usr/bin/ditto "${source_app}" "${staged_app}"
"${verify_script}" "${staged_app}"

if ! terminate_running_app; then
  echo "the running Grok Computer Use app did not terminate; installation was not changed" >&2
  exit 1
fi

backup_app=""
if [[ -e "${target_app}" ]]; then
  if [[ ! -d "${target_app}" ]]; then
    echo "refusing to replace non-directory ${target_app}" >&2
    exit 1
  fi
  backup_app="${applications_dir}/Grok Computer Use.app.backup-$(/bin/date -u +%Y%m%dT%H%M%SZ)"
  /bin/mv "${target_app}" "${backup_app}"
fi
restore_previous_app() {
  if [[ -e "${target_app}" ]]; then
    /bin/mv "${target_app}" "${staging_root}/rejected-Grok Computer Use.app"
  fi
  if [[ -n "${backup_app}" && -d "${backup_app}" ]]; then
    /bin/mv "${backup_app}" "${target_app}"
  fi
}
if ! /bin/mv "${staged_app}" "${target_app}"; then
  restore_previous_app
  exit 1
fi
echo "launching ${target_app}; approve any macOS permission or Keychain prompts"
if ! "${verify_script}" "${target_app}" \
  || ! /usr/bin/open "${target_app}"; then
  restore_previous_app
  echo "installation verification or launch failed; the previous app was restored" >&2
  exit 1
fi

# Do not commit the upgrade until the new companion is accepting connections
# and remains healthy after its synchronous startup checks. A startup failure
# restores the previous bundle; a successful startup no longer leaves a
# duplicate app for LaunchServices or Spotlight to discover.
agent_ready=0
echo "waiting up to 60 seconds for Grok Computer Use to finish first-launch setup"
for _ in {1..600}; do
  if agent_is_ready; then
    agent_ready=1
    break
  fi
  /bin/sleep 0.1
done
/bin/sleep 1
if [[ "${agent_ready}" -ne 1 ]] || ! agent_is_ready; then
  if [[ -z "${backup_app}" ]]; then
    echo "Grok Computer Use was installed but is still waiting for first-launch approval; it was left at ${target_app}" >&2
    exit 1
  fi
  if ! terminate_running_app; then
    echo "the replacement app did not become ready and could not be stopped; the previous app remains at ${backup_app}" >&2
    exit 1
  fi
  restore_previous_app
  /usr/bin/open -gj "${target_app}" || true
  echo "the new Grok Computer Use app did not become ready; the previous app was restored" >&2
  exit 1
fi

echo "installed ${target_app}"
if [[ -n "${backup_app}" && -d "${backup_app}" ]]; then
  /bin/rm -rf "${backup_app}"
  echo "removed temporary rollback copy ${backup_app}"
fi
