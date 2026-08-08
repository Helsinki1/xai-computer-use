#!/usr/bin/env python3
"""Minimal MCP smoke test for the Linux relay.

This uses the documented development override so the relay can trust its
Python parent process without needing the full Grok host.
"""

from __future__ import annotations

import json
import os
import subprocess
import sys
from pathlib import Path


def send(proc: subprocess.Popen[str], payload: dict) -> dict | None:
    assert proc.stdin is not None
    assert proc.stdout is not None
    proc.stdin.write(json.dumps(payload) + "\n")
    proc.stdin.flush()
    line = proc.stdout.readline().strip()
    return json.loads(line) if line else None


def main() -> int:
    relay = Path.home() / ".local/libexec/grok-computer-use/grok-computer-use-mcp"
    env = os.environ.copy()
    env.setdefault("GROK_COMPUTER_USE_PARENT_EXECUTABLES", sys.executable)

    proc = subprocess.Popen(
        [str(relay)],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        env=env,
    )
    try:
        init = send(
            proc,
            {
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": {
                    "protocolVersion": "2025-06-18",
                    "capabilities": {},
                    "clientInfo": {"name": "modal-smoke", "version": "0.1.0"},
                },
            },
        )
        send(
            proc,
            {
                "jsonrpc": "2.0",
                "method": "notifications/initialized",
                "params": {},
            },
        )
        tools = send(proc, {"jsonrpc": "2.0", "id": 2, "method": "tools/list"})
        result = {
            "initialize": init,
            "tools": tools,
        }
        print(json.dumps(result, indent=2))
        return 0
    finally:
        try:
            proc.terminate()
            proc.wait(timeout=5)
        except subprocess.TimeoutExpired:
            proc.kill()
        if proc.stderr is not None:
            stderr = proc.stderr.read().strip()
            if stderr:
                print(stderr, file=sys.stderr)


if __name__ == "__main__":
    raise SystemExit(main())
