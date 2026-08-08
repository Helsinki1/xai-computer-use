#!/usr/bin/env bash

set -euo pipefail

component_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
configuration="${1:-release}"
identity="${GROK_COMPUTER_USE_CODESIGN_IDENTITY:--}"
version="${GROK_COMPUTER_USE_VERSION:-0.1.0}"
build_number="${GROK_COMPUTER_USE_BUILD_NUMBER:-1}"

if [[ "$(uname -s)" != "Darwin" || "$(uname -m)" != "arm64" ]]; then
  echo "build-app.sh requires Apple Silicon macOS" >&2
  exit 1
fi
major_version="$(/usr/bin/sw_vers -productVersion | /usr/bin/cut -d. -f1)"
if [[ "${major_version}" -lt 14 ]]; then
  echo "build-app.sh requires macOS 14 or newer" >&2
  exit 1
fi
if [[ "${configuration}" != "debug" && "${configuration}" != "release" ]]; then
  echo "usage: $0 [debug|release]" >&2
  exit 1
fi

swift build --package-path "${component_root}" -c "${configuration}" --arch arm64 --product GrokComputerUseApp
swift build --package-path "${component_root}" -c "${configuration}" --arch arm64 --product grok-computer-use-mcp
binary_dir="$(swift build --package-path "${component_root}" -c "${configuration}" --arch arm64 --show-bin-path)"

dist_dir="${component_root}/dist"
app_path="${dist_dir}/Grok Computer Use.app"
contents_path="${app_path}/Contents"
macos_path="${contents_path}/MacOS"
resources_path="${contents_path}/Resources"

if [[ -e "${app_path}" && ! -d "${app_path}" ]]; then
  echo "refusing to replace non-directory ${app_path}" >&2
  exit 1
fi
/bin/rm -rf "${app_path}"
/bin/mkdir -p "${macos_path}" "${resources_path}"
/bin/cp "${binary_dir}/GrokComputerUseApp" "${macos_path}/GrokComputerUseApp"
/bin/cp "${binary_dir}/grok-computer-use-mcp" "${macos_path}/grok-computer-use-mcp"
/bin/cp "${component_root}/Resources/Info.plist" "${contents_path}/Info.plist"
/usr/libexec/PlistBuddy -c "Set :CFBundleShortVersionString ${version}" "${contents_path}/Info.plist"
/usr/libexec/PlistBuddy -c "Set :CFBundleVersion ${build_number}" "${contents_path}/Info.plist"
/usr/bin/plutil -lint "${contents_path}/Info.plist" >/dev/null
/bin/chmod 0755 "${macos_path}/GrokComputerUseApp" "${macos_path}/grok-computer-use-mcp"

sign_args=(--force --sign "${identity}")
if [[ "${identity}" != "-" ]]; then
  sign_args+=(--options runtime --timestamp)
fi
/usr/bin/codesign "${sign_args[@]}" \
  --identifier com.xai.grok.computer-use.mcp \
  "${macos_path}/grok-computer-use-mcp"
/usr/bin/codesign "${sign_args[@]}" \
  --entitlements "${component_root}/Resources/GrokComputerUse.entitlements" \
  "${app_path}"
/usr/bin/codesign --verify --deep --strict --verbose=2 "${app_path}"

echo "built ${app_path}"
if [[ "${identity}" == "-" ]]; then
  echo "warning: ad-hoc signatures are for local tests only; Grok's trusted profile requires a Gatekeeper-valid stable identity" >&2
fi
