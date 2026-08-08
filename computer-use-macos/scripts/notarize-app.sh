#!/usr/bin/env bash

set -euo pipefail

component_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
app_path="${1:-${component_root}/dist/Grok Computer Use.app}"
notary_profile="${GROK_COMPUTER_USE_NOTARY_PROFILE:-}"

if [[ "$(uname -s)" != "Darwin" || "$(uname -m)" != "arm64" ]]; then
  echo "notarize-app.sh requires Apple Silicon macOS" >&2
  exit 1
fi
if [[ -z "${notary_profile}" ]]; then
  echo "notarize-app.sh requires GROK_COMPUTER_USE_NOTARY_PROFILE" >&2
  exit 1
fi
if [[ ! -d "${app_path}" ]]; then
  echo "app bundle not found: ${app_path}" >&2
  exit 1
fi

/usr/bin/codesign --verify --deep --strict --verbose=2 "${app_path}"

staging_root="$(/usr/bin/mktemp -d "${TMPDIR:-/tmp}/grok-computer-use.notary.XXXXXX")"
cleanup() {
  /bin/rm -rf "${staging_root}"
}
trap cleanup EXIT

archive_path="${staging_root}/Grok Computer Use.zip"
/usr/bin/ditto -c -k --keepParent "${app_path}" "${archive_path}"
/usr/bin/xcrun notarytool submit \
  --keychain-profile "${notary_profile}" \
  --wait \
  "${archive_path}"
/usr/bin/xcrun stapler staple "${app_path}"
/usr/bin/xcrun stapler validate "${app_path}"

echo "notarized and stapled ${app_path}"
