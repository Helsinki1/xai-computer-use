#!/usr/bin/env bash

set -euo pipefail

component_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
repo_root="$(cd "${component_root}/.." && pwd)"
grok_binary="${repo_root}/target/debug/xai-grok-pager"
identity="${GROK_COMPUTER_USE_CODESIGN_IDENTITY:--}"

if [[ ! -x "${grok_binary}" ]]; then
  echo "build Grok first: cargo build -p xai-grok-pager-bin" >&2
  exit 1
fi

# The debug relay accepts only the exact allowlisted host name/identifier. A
# stable local identity keeps the host aligned with an app built using the same
# identity; the ad-hoc fallback remains available when no identity is configured.
current_identifier="$(/usr/bin/codesign -d --verbose=4 "${grok_binary}" 2>&1 \
  | /usr/bin/sed -n 's/^Identifier=//p' \
  | /usr/bin/head -n 1 || true)"
if [[ "${identity}" != "-" || "${current_identifier}" != "xai-grok-pager" ]]; then
  /usr/bin/codesign --force --sign "${identity}" --identifier xai-grok-pager "${grok_binary}" >/dev/null
fi
export GROK_COMPUTER_USE_LOCAL_DEV=1
exec "${grok_binary}" "$@"
