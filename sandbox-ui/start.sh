#!/usr/bin/env bash
set -euo pipefail

PORT="${1:-4173}"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

cd "$SCRIPT_DIR"
python3 -m http.server "$PORT"
