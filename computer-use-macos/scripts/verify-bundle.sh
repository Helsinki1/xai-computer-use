#!/usr/bin/env bash

set -euo pipefail

app_path="${1:?usage: verify-bundle.sh <app-path>}"
relay_path="${app_path}/Contents/MacOS/grok-computer-use-mcp"
expected_app_identifier="com.xai.grok.computer-use"
expected_relay_identifier="com.xai.grok.computer-use.mcp"

[[ "$(uname -s)" == "Darwin" ]]
[[ "$(uname -m)" == "arm64" ]]
[[ -d "${app_path}" && -x "${relay_path}" ]]

signature_field() {
  local path="$1"
  local field="$2"
  /usr/bin/codesign -d --verbose=4 "${path}" 2>&1 \
    | /usr/bin/sed -n "s/^${field}=//p" \
    | /usr/bin/head -n 1
}

/usr/bin/codesign --verify --deep --strict --verbose=2 "${app_path}"
/usr/bin/codesign --verify --strict --verbose=2 "${relay_path}"

app_identifier="$(signature_field "${app_path}" Identifier)"
relay_identifier="$(signature_field "${relay_path}" Identifier)"
app_team="$(signature_field "${app_path}" TeamIdentifier)"
relay_team="$(signature_field "${relay_path}" TeamIdentifier)"

[[ "${app_identifier}" == "${expected_app_identifier}" ]]
[[ "${relay_identifier}" == "${expected_relay_identifier}" ]]
[[ -n "${app_team}" && "${app_team}" != "not set" ]]
[[ "${relay_team}" == "${app_team}" ]]
if [[ -n "${GROK_COMPUTER_USE_EXPECTED_TEAM_ID:-}" ]]; then
  [[ "${app_team}" == "${GROK_COMPUTER_USE_EXPECTED_TEAM_ID}" ]]
fi

/usr/bin/codesign -d --verbose=4 "${app_path}" 2>&1 \
  | /usr/bin/grep -Eq '^CodeDirectory .* flags=.*\(.*runtime.*\)'
/usr/bin/codesign -d --verbose=4 "${relay_path}" 2>&1 \
  | /usr/bin/grep -Eq '^CodeDirectory .* flags=.*\(.*runtime.*\)'
/usr/bin/xcrun stapler validate "${app_path}"
/usr/sbin/spctl --assess --type execute --verbose=2 "${app_path}"

echo "verified signed and notarized bundle ${app_path}"
