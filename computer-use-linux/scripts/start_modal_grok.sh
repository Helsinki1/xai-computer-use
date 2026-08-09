#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="${HOME}/xai-computer-use"
GROK_BIN="${REPO_ROOT}/target/release/xai-grok-pager"

if [[ ! -x "${GROK_BIN}" ]]; then
    printf 'missing Grok binary: %s\n' "${GROK_BIN}" >&2
    exit 1
fi

cd "${REPO_ROOT}"

printf 'Preparing Grok computer use inside Modal...\n'
"${GROK_BIN}" computer-use status || true
"${GROK_BIN}" computer-use enable || true

if [[ -z "${XAI_API_KEY:-}" ]]; then
    printf '\nXAI_API_KEY is not set in this container.\n'
    printf 'You can still use the terminal, but Grok may prompt for auth or fail to connect.\n\n'
fi

exec "${GROK_BIN}"
