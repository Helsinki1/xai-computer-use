#!/usr/bin/env bash

# Prepare the local-debug computer-use app and launch Grok. Cargo and Swift
# perform incremental builds, so repeated invocations do not rebuild unchanged
# targets. Pass --rebuild to reinstall the app unconditionally, or
# --no-rebuild to require already-current artifacts.
set -euo pipefail

component_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
repo_root="$(cd "${component_root}/.." && pwd)"
installed_app="${HOME}/Applications/Grok Computer Use.app"
app_binary="${installed_app}/Contents/MacOS/GrokComputerUseApp"
rebuild_mode="auto"
export GROK_COMPUTER_USE_LOCAL_DEV=1

usage() {
  echo "usage: $0 [--rebuild|--no-rebuild] [--] [grok arguments...]" >&2
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --rebuild)
      rebuild_mode="always"
      shift
      ;;
    --no-rebuild)
      rebuild_mode="never"
      shift
      ;;
    --help|-h)
      usage
      exit 0
      ;;
    --)
      shift
      break
      ;;
    *)
      break
      ;;
  esac
done

if [[ -z "${XAI_API_KEY:-}" ]]; then
  read -r -s -p "XAI_API_KEY: " XAI_API_KEY
  echo >&2
  export XAI_API_KEY
fi

app_is_stale() {
  [[ ! -x "${app_binary}" ]] && return 0
  find "${component_root}/Sources" "${component_root}/Resources" \
    "${component_root}/Package.swift" "${component_root}/scripts/build-app.sh" \
    -type f -newer "${app_binary}" -print -quit | grep -q .
}

case "${rebuild_mode}" in
  always)
    "${component_root}/scripts/install-app.sh" --build
    ;;
  auto)
    if app_is_stale; then
      "${component_root}/scripts/install-app.sh" --build
    fi
    ;;
  never)
    if app_is_stale; then
      echo "Grok Computer Use is missing or stale; rerun without --no-rebuild" >&2
      exit 1
    fi
    ;;
esac

if [[ "${rebuild_mode}" != "never" ]]; then
  (cd "${repo_root}" && cargo build -p xai-grok-pager-bin)
fi
exec "${component_root}/scripts/run-local-grok.sh" "$@"
