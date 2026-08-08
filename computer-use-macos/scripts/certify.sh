#!/usr/bin/env bash

set -euo pipefail

component_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
identity="${GROK_COMPUTER_USE_CODESIGN_IDENTITY:-}"
notary_profile="${GROK_COMPUTER_USE_NOTARY_PROFILE:-}"
expected_team="${GROK_COMPUTER_USE_EXPECTED_TEAM_ID:-}"

if [[ "$(uname -s)" != "Darwin" || "$(uname -m)" != "arm64" ]]; then
  echo "certify.sh requires an Apple Silicon macOS 14+ runner" >&2
  exit 1
fi
if [[ -z "${identity}" || "${identity}" == "-" ]]; then
  echo "certify.sh requires a stable Developer ID signing identity" >&2
  exit 1
fi
if [[ -z "${notary_profile}" ]]; then
  echo "certify.sh requires GROK_COMPUTER_USE_NOTARY_PROFILE" >&2
  exit 1
fi
if [[ -z "${expected_team}" ]]; then
  echo "certify.sh requires GROK_COMPUTER_USE_EXPECTED_TEAM_ID" >&2
  exit 1
fi

for script in "${component_root}"/scripts/*.sh; do
  /bin/bash -n "${script}"
done
/usr/bin/plutil -lint "${component_root}/Resources/Info.plist" >/dev/null
/usr/bin/plutil -lint "${component_root}/Resources/GrokComputerUse.entitlements" >/dev/null
swift test --package-path "${component_root}" --parallel
GROK_COMPUTER_USE_CODESIGN_IDENTITY="${identity}" \
  "${component_root}/scripts/build-app.sh" release
GROK_COMPUTER_USE_NOTARY_PROFILE="${notary_profile}" \
  "${component_root}/scripts/notarize-app.sh" "${component_root}/dist/Grok Computer Use.app"
"${component_root}/scripts/verify-bundle.sh" "${component_root}/dist/Grok Computer Use.app"

if [[ "${GROK_COMPUTER_USE_RUN_RUST_CONTRACT_TESTS:-0}" == "1" ]]; then
  repo_root="$(cd "${component_root}/.." && pwd)"
  cargo test --locked --manifest-path "${repo_root}/Cargo.toml" -p xai-grok-mcp --lib computer_use
fi

echo "computer-use macOS release certification passed"
