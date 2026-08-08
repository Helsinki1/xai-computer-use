#!/usr/bin/env bash

set -euo pipefail

component_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
app_path="${HOME}/Applications/Grok Computer Use.app"
relay_path="${app_path}/Contents/MacOS/grok-computer-use-mcp"

[[ "$(uname -s)" == "Darwin" ]]
[[ "$(uname -m)" == "arm64" ]]
[[ -d "${app_path}" && -x "${relay_path}" ]]

path="${relay_path}"
while [[ "${path}" != "/" ]]; do
  if [[ -L "${path}" ]]; then
    echo "symbolic link in trusted path: ${path}" >&2
    exit 1
  fi
  path="$(/usr/bin/dirname "${path}")"
done

/usr/bin/plutil -lint "${app_path}/Contents/Info.plist" >/dev/null
"${component_root}/scripts/verify-bundle.sh" "${app_path}"

echo "verified ${app_path}"
