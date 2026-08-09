#!/usr/bin/env python3
"""Visible browser demo driven through the computer-use MCP relay.

The flow is intentionally conservative: it shows the system controlling a real
browser, searching Google, and opening a reservation-site landing page, but it
does not submit a live reservation.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import subprocess
import sys
import time
import uuid
from typing import Any


PROTOCOL_VERSION = "2025-06-18"
BROWSER_PATTERNS = ("chrom", "chrome")
DEFAULT_SEARCH = "best italian restaurants in manhattan opentable"
DEFAULT_RESERVATION_URL = (
    "https://www.opentable.com/s?dateTime=2026-08-15T19%3A00%3A00&covers=2&term=Manhattan"
)
DEFAULT_STEPS = [
    {"action": "navigate", "text": "https://www.google.com", "settle": 4},
    {"action": "navigate", "text": DEFAULT_SEARCH, "settle": 6},
    {"action": "navigate", "text": DEFAULT_RESERVATION_URL, "settle": 8},
]


def trusted_meta(tool_name: str) -> dict[str, Any]:
    return {
        "xai/computer-use-v2": {
            "profile": "computer-use-v2",
            "logical_call_id": str(uuid.uuid4()),
            "session_id": str(uuid.uuid4()),
            "workflow_id": "modal-restaurant-demo",
            "action_id": str(uuid.uuid4()),
            "tool_name": tool_name,
        }
    }


def send(proc: subprocess.Popen[str], payload: dict[str, Any]) -> dict[str, Any] | None:
    assert proc.stdin is not None
    assert proc.stdout is not None
    proc.stdin.write(json.dumps(payload) + "\n")
    proc.stdin.flush()
    line = proc.stdout.readline().strip()
    return json.loads(line) if line else None


class RelayClient:
    def __init__(self) -> None:
        relay = os.path.expanduser("~/.local/libexec/grok-computer-use/grok-computer-use-mcp")
        env = os.environ.copy()
        env.setdefault("GROK_COMPUTER_USE_PARENT_EXECUTABLES", sys.executable)
        self.proc = subprocess.Popen(
            [relay],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            env=env,
        )
        init = send(
            self.proc,
            {
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": {
                    "protocolVersion": PROTOCOL_VERSION,
                    "capabilities": {},
                    "clientInfo": {"name": "modal-restaurant-demo", "version": "0.1.0"},
                },
            },
        )
        if not init or "error" in init:
            raise RuntimeError(f"relay initialize failed: {init}")
        send(
            self.proc,
            {
                "jsonrpc": "2.0",
                "method": "notifications/initialized",
                "params": {},
            },
        )
        self.next_id = 2

    def close(self) -> None:
        try:
            self.proc.terminate()
            self.proc.wait(timeout=5)
        except subprocess.TimeoutExpired:
            self.proc.kill()
        if self.proc.stderr is not None:
            stderr = self.proc.stderr.read().strip()
            if stderr:
                print(stderr, file=sys.stderr)

    def call_tool(self, name: str, arguments: dict[str, Any]) -> dict[str, Any]:
        payload = {
            "jsonrpc": "2.0",
            "id": self.next_id,
            "method": "tools/call",
            "params": {
                "name": name,
                "arguments": arguments,
                "_meta": trusted_meta(name),
            },
        }
        self.next_id += 1
        response = send(self.proc, payload)
        if not response:
            raise RuntimeError(f"empty MCP response for {name}")
        if "error" in response:
            raise RuntimeError(f"MCP error for {name}: {response['error']}")
        result = response["result"]
        if result.get("isError"):
            raise RuntimeError(f"tool {name} failed: {result}")
        return result


def result_text(result: dict[str, Any]) -> str:
    for item in result.get("content", []):
        if item.get("type") == "text":
            return item.get("text", "")
    return ""


def discover_browser_bundle(client: RelayClient, retries: int = 30, delay: float = 1.0) -> str:
    for _ in range(retries):
        result = client.call_tool("list_apps", {})
        text = result_text(result)
        for line in text.splitlines():
            if "bundle_id=" not in line:
                continue
            bundle_match = re.search(r"bundle_id=([^ ]+)", line)
            if not bundle_match:
                continue
            bundle_id = bundle_match.group(1)
            name = line.rsplit("name=", 1)[-1].lower()
            lowered_bundle = bundle_id.lower()
            if any(pattern in lowered_bundle or pattern in name for pattern in BROWSER_PATTERNS):
                return bundle_id
        time.sleep(delay)
    raise RuntimeError("browser application did not appear in list_apps")


def get_state(client: RelayClient, bundle_id: str) -> dict[str, Any]:
    return client.call_tool("get_app_state", {"bundle_id": bundle_id})


def carrier_from_state(state: dict[str, Any]) -> dict[str, Any]:
    return state["_meta"]["xai/computer-use-v2"]


def attest(client: RelayClient, carrier: dict[str, Any]) -> None:
    client.call_tool(
        "attest_snapshot_delivery",
        {
            "snapshot_id": carrier["snapshot_id"],
            "attestation_id": carrier["attestation_id"],
            "png_sha256": carrier["png_sha256"],
        },
    )


def snapshot_center(carrier: dict[str, Any]) -> tuple[float, float]:
    return carrier["png_width_px"] / 2.0, carrier["png_height_px"] / 2.0


def click_center(client: RelayClient, bundle_id: str) -> None:
    state = get_state(client, bundle_id)
    carrier = carrier_from_state(state)
    attest(client, carrier)
    x_px, y_px = snapshot_center(carrier)
    client.call_tool(
        "click",
        {
            "snapshot_id": carrier["snapshot_id"],
            "target": {"kind": "pixel", "x_px": x_px, "y_px": y_px},
        },
    )


def press_key(client: RelayClient, bundle_id: str, key: str, modifiers: list[str] | None = None) -> None:
    state = get_state(client, bundle_id)
    carrier = carrier_from_state(state)
    attest(client, carrier)
    arguments: dict[str, Any] = {
        "snapshot_id": carrier["snapshot_id"],
        "key": key,
    }
    if modifiers:
        arguments["modifiers"] = modifiers
    client.call_tool("press_key", arguments)


def type_text(client: RelayClient, bundle_id: str, text: str) -> None:
    state = get_state(client, bundle_id)
    carrier = carrier_from_state(state)
    attest(client, carrier)
    client.call_tool(
        "type_text",
        {
            "snapshot_id": carrier["snapshot_id"],
            "text": text,
        },
    )


def navigate_by_address_bar(client: RelayClient, bundle_id: str, text: str, settle: float = 5.0) -> None:
    click_center(client, bundle_id)
    time.sleep(1)
    press_key(client, bundle_id, "l", ["control"])
    time.sleep(0.5)
    type_text(client, bundle_id, text)
    time.sleep(0.5)
    press_key(client, bundle_id, "Return")
    time.sleep(settle)


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--startup-delay", type=float, default=None)
    parser.add_argument("--search-query", default=None)
    parser.add_argument("--final-url", default=None)
    parser.add_argument(
        "--steps-json",
        default=None,
        help="JSON array of actions. Supported actions: navigate, wait, click_center.",
    )
    return parser.parse_args()


def planned_steps(args: argparse.Namespace) -> list[dict[str, Any]]:
    if args.steps_json:
        steps = json.loads(args.steps_json)
        if not isinstance(steps, list):
            raise RuntimeError("--steps-json must decode to a JSON array")
        return steps
    query = args.search_query or os.environ.get("DEMO_SEARCH_QUERY", DEFAULT_SEARCH)
    final_url = args.final_url or os.environ.get("DEMO_RESERVATION_URL", DEFAULT_RESERVATION_URL)
    return [
        {"action": "navigate", "text": "https://www.google.com", "settle": 4},
        {"action": "navigate", "text": query, "settle": 6},
        {"action": "navigate", "text": final_url, "settle": 8},
    ]


def run_step(client: RelayClient, bundle_id: str, step: dict[str, Any]) -> None:
    action = step.get("action")
    if action == "navigate":
        text = step.get("text")
        if not isinstance(text, str) or not text:
            raise RuntimeError(f"invalid navigate step: {step}")
        navigate_by_address_bar(client, bundle_id, text, float(step.get("settle", 5)))
        return
    if action == "wait":
        time.sleep(float(step.get("seconds", 2)))
        return
    if action == "click_center":
        click_center(client, bundle_id)
        time.sleep(float(step.get("settle", 1)))
        return
    raise RuntimeError(f"unsupported demo action: {action}")


def main() -> int:
    args = parse_args()
    startup_delay = args.startup_delay
    if startup_delay is None:
        startup_delay = float(os.environ.get("DEMO_STARTUP_DELAY_SECONDS", "10"))
    steps = planned_steps(args)
    print(f"waiting {startup_delay:.1f}s for browser startup")
    time.sleep(startup_delay)

    client = RelayClient()
    try:
        bundle_id = discover_browser_bundle(client)
        print(f"controlling browser bundle_id={bundle_id}")
        for step in steps:
            run_step(client, bundle_id, step)
        print("demo finished")
        return 0
    finally:
        client.close()


if __name__ == "__main__":
    raise SystemExit(main())
