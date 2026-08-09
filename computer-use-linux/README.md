# Grok Computer Use for Linux (Ubuntu / X11 MVP)

This subtree is the Linux port of `computer-use-macos/`: a native desktop
daemon that owns capture, input dispatch, leases, snapshots, and durable
receipts, plus a stateless stdio MCP relay. The v2 agent protocol, tool
catalog, pixel coordinate contract, and lease/snapshot/receipt semantics are
identical to the macOS implementation; only the native bindings and the local
trust model differ. `LINUX_MVP_CHECKLIST.md` tracks the frozen acceptance
contract and remaining work.

## Supported environment (MVP)

- Ubuntu 22.04/24.04 with an **X11** session (`echo $XDG_SESSION_TYPE` must be
  `x11`) and an EWMH-compliant window manager.
- Explicit non-goals: Wayland, compositor independence, macOS-equivalent
  code-signing trust, distro-agnostic packaging.

## Layout

- `computer-use-core/` — shared protocol/runtime crate mirroring
  `ComputerUseCore` (agent protocol v2, HMAC session auth, wire framing, tool
  catalog, coordinate mapper, lease/snapshot/receipt runtime, MCP server).
- `grok-computer-use-daemon/` — the native daemon mirroring
  `GrokComputerUseApp`: singleton lease, SQLite receipt store, Unix socket
  server, peer verification, X11/XTest desktop driver.
- `grok-computer-use-mcp/` — the stdio MCP relay mirroring
  `GrokComputerUseMCP`: parent verification, one authenticated daemon
  connection per process, fail-closed poisoning, no reconnect or replay.

## Build, test, install

```sh
cd computer-use-linux
cargo test
cargo build --release
scripts/install.sh          # installs to ~/.local/libexec/grok-computer-use
grok-computer-use-daemon &  # or enable the systemd user unit the script offers
```

Fixed paths (the relay and daemon refuse substitutes):

- Binaries: `~/.local/libexec/grok-computer-use/{grok-computer-use-daemon,grok-computer-use-mcp}`
- Socket: `$XDG_RUNTIME_DIR/grok-computer-use/agent-v2.sock` (dir 0700, socket 0600)
- Receipts + HMAC key: `~/.local/share/grok-computer-use/receipts/` (0700/0600)

## Coordinate contract (unchanged)

Model coordinates are continuous PNG edge coordinates with a top-left origin;
`global = capture_origin + coordinate * capture_extent / png_extent` with
independent X/Y ratios and no rounding, clamping, half-pixel offset, Y flip,
scale inference, or identity fallback. On X11, global points are root-window
pixels. The PNG is at most 1,280 px per side, 1,638,400 px total, and 900,000
bytes; capture fails closed otherwise. The integer device grid is entered only
at XTest injection time, by flooring into the containing pixel cell.

## Linux mapping decisions

- `bundle_id` carries the WM_CLASS class name (the stable X11 application
  identity); `window_id` is the X11 window identifier; `pid` comes from
  `_NET_WM_PID`.
- Capture prefers the XComposite backing pixmap (exact even when partially
  obscured) and falls back to a direct window `GetImage`.
- Every input dispatch revalidates the exact captured window first: same
  window identifier, same process, same global geometry, still viewable.
- Key names accept the macOS vocabulary (`command`→Super, `option`→Alt); `fn`
  has no X11 equivalent and fails closed. Text entry uses the keyboard map,
  temporarily remapping one spare keycode for symbols outside it.

## MVP observation reduction (phase 2: AT-SPI2)

`get_app_state` returns exact geometry, the bounded screenshot, and the window
title; the accessibility tree is a one-line window summary and `elements` is
empty. Consequently `perform_secondary_action`, `set_value`, element-targeted
`click`, and element-targeted `scroll` return `invalid_arguments` until the
AT-SPI2 walk lands (checklist §4). Pixel-space `click`, `drag`, `type_text`,
and `press_key` are fully functional.

## Security delta versus macOS (read before deploying)

The macOS build anchors trust in code signatures, notarization, audit tokens,
and the Keychain. Linux has no equivalent, so this MVP deliberately ships a
**same-user trust model**:

- Peer verification is `SO_PEERCRED` same-uid plus a `/proc/<pid>/exe`
  allowlist (fixed relay path; `GROK_COMPUTER_USE_EXTRA_PEER_EXECUTABLE`
  extends it for dev/CI). `/proc` resolution has an inherent TOCTOU window.
- The relay requires its direct parent to be a same-user allowlisted Grok host
  (`GROK_COMPUTER_USE_PARENT_EXECUTABLES` overrides for dev/CI); there is no
  team-identity equivalent.
- The receipt HMAC key is a mode-0600 file, not a Keychain item; SQLite runs
  WAL + `synchronous=FULL` but Linux has no `F_FULLFSYNC`, so durability
  depends on the storage stack honoring fsync.
- The process singleton is an abstract-namespace socket bind plus `flock`,
  acquired before any durable state is touched.

Everything above the native layer is unweakened: per-connection HMAC-SHA256
session handshakes with monotonic sequence numbers, single-use delivery-
attested snapshots, fenced leases, authenticated receipts with the
`prepared -> dispatched -> applied | rejected | outcome_unknown` lifecycle,
no-retry on uncertain dispatch, 2 MiB frames, 64-connection cap, and screenshot
bytes confined to the protected `xai/computer-use-v2` carrier. Receipts are
retained indefinitely (same v2 limitation as macOS).

## Remaining integration work

The Rust host currently gates the reserved capability on the macOS-only
`trusted_relay_path()` in
`crates/codegen/xai-grok-shell/src/util/computer_use.rs`; enabling
`grok computer-use enable` on Linux requires the host-side wiring and the CI
job listed in `LINUX_MVP_CHECKLIST.md` §5.
