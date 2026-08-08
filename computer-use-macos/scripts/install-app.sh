#!/usr/bin/env bash

set -euo pipefail

component_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
source_app="${component_root}/dist/Grok Computer Use.app"
applications_dir="${HOME}/Applications"
target_app="${applications_dir}/Grok Computer Use.app"
local_development="${GROK_COMPUTER_USE_LOCAL_DEV:-0}"

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "install-app.sh requires macOS" >&2
  exit 1
fi
if [[ "$(uname -m)" != "arm64" && "${local_development}" != "1" ]]; then
  echo "Intel installation requires GROK_COMPUTER_USE_LOCAL_DEV=1" >&2
  exit 1
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

if /usr/bin/pgrep -u "$(/usr/bin/id -u)" -x GrokComputerUseApp >/dev/null; then
  /usr/bin/pkill -TERM -u "$(/usr/bin/id -u)" -x GrokComputerUseApp
  for _ in {1..50}; do
    if ! /usr/bin/pgrep -u "$(/usr/bin/id -u)" -x GrokComputerUseApp >/dev/null; then
      break
    fi
    /bin/sleep 0.1
  done
  if /usr/bin/pgrep -u "$(/usr/bin/id -u)" -x GrokComputerUseApp >/dev/null; then
    echo "the running Grok Computer Use app did not terminate; installation was not changed" >&2
    exit 1
  fi
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
if ! "${verify_script}" "${target_app}" \
  || ! /usr/bin/open -gj "${target_app}"; then
  restore_previous_app
  echo "installation verification or launch failed; the previous app was restored" >&2
  exit 1
fi

echo "installed ${target_app}"
if [[ -n "${backup_app}" ]]; then
  echo "previous version retained at ${backup_app}"
fi
