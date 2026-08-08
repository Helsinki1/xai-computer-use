#!/usr/bin/env bash

set -euo pipefail

app_path="${1:?usage: verify-local-bundle.sh <app-path>}"
app_executable="${app_path}/Contents/MacOS/GrokComputerUseApp"
relay_path="${app_path}/Contents/MacOS/grok-computer-use-mcp"
machine_arch="$(uname -m)"

[[ "$(uname -s)" == "Darwin" ]]
[[ -d "${app_path}" && -x "${app_executable}" && -x "${relay_path}" ]]
/usr/bin/lipo "${app_executable}" -verify_arch "${machine_arch}"
/usr/bin/lipo "${relay_path}" -verify_arch "${machine_arch}"
/usr/bin/codesign --verify --deep --strict --verbose=2 "${app_path}"
/usr/bin/codesign --verify --strict --verbose=2 "${relay_path}"

signature_field() {
  local path="$1"
  local field="$2"
  /usr/bin/codesign -d --verbose=4 "${path}" 2>&1 \
    | /usr/bin/sed -n "s/^${field}=//p" \
    | /usr/bin/head -n 1
}

[[ "$(signature_field "${app_path}" Identifier)" == "com.xai.grok.computer-use" ]]
[[ "$(signature_field "${relay_path}" Identifier)" == "com.xai.grok.computer-use.mcp" ]]

echo "verified local debug bundle ${app_path}"
