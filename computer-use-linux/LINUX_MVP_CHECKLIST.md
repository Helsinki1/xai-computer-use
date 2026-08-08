# Linux computer-use MVP — frozen acceptance contract and tracked checklist

Target: feature parity with the macOS implementation on **one supported stack:
Ubuntu 22.04/24.04 on X11**. Generic Wayland, compositor independence,
macOS-equivalent code-signing trust, and distro-agnostic packaging are explicit
non-goals for the MVP (see `COMPUTER_USE_IMPLEMENTATION.md` for the macOS
reference and the plan summary at the bottom of this file).

Legend: `[ ]` todo · `[x]` done · `[~]` in progress · `(P2)` deliberately
deferred to phase 2.

## 1. Frozen platform-agnostic contract (acceptance target)

These invariants are copied from the macOS implementation and MUST hold
identically on Linux. They are the definition of "parity".

### Protocol (v2, unchanged)

- [x] Agent wire protocol v2: 8-byte big-endian length prefix + JSON payload,
      2 MiB frame cap, strict allowed/required key validation before decode.
- [x] Request kinds: `initialize`, `ping`, `call_tool`, `action_outcome`,
      `attest_snapshot_delivery`, `invalidate_session`, `lease_heartbeat`,
      `release_operation`, with the exact per-kind field validation rules of
      `AgentProtocol.swift`.
- [x] Fresh per-connection HMAC-SHA256 session handshake: 32-byte client
      secret, server session identifier, `initialize-v2` proof, then
      `request-v2` / `response-v2` domain-separated tags over the canonical
      JSON payload with monotonic sequence numbers (`sequence > 0`, strictly
      increasing, replay and cross-connection messages rejected).
- [x] Action identifiers: non-empty, ≤ 256 UTF-8 bytes, no control characters;
      required on action tools, forbidden on read-only tools.

### Tool catalog (v2, unchanged)

- [x] Visible tools: `list_apps`, `get_app_state`, `click`,
      `perform_secondary_action`, `scroll`, `drag`, `type_text`, `press_key`,
      `set_value` — with the exact v2 JSON schemas and annotations.
- [x] Hidden lifecycle tools: `attest_snapshot_delivery`, `invalidate_session`,
      `lease_heartbeat`, `release_operation`.
- [x] Unknown-argument rejection: every tool call fails closed on fields not in
      its schema (ArgumentReader `finish()` semantics).

### Pixel coordinate contract (unchanged)

- [x] Model coordinates are continuous PNG edge coordinates, top-left origin,
      `0 <= x < png_width`, `0 <= y < png_height`.
- [x] `global = capture_origin + coordinate * capture_extent / png_extent`
      with independent X/Y ratios; **no** rounding, clamping, half-pixel
      offset, Y flip, scale inference, or identity fallback.
- [x] Capture records the exact window rectangle in global coordinates and the
      exact final PNG dimensions; incomplete geometry fails closed.
- [x] PNG bounds: ≤ 1,280 px per side, ≤ 1,638,400 px total, ≤ 900,000 bytes;
      capture fails closed if the contract cannot be met.

### Lease / snapshot / receipt semantics (unchanged)

- [x] One fenced desktop lease per host; heartbeats renew it; a foreign owner
      gets `desktop_busy` with a retry-after hint.
- [x] Snapshots are server-generated, session-bound, delivery-attested
      (`attest_snapshot_delivery` must present the attestation identifier and
      the exact PNG SHA-256), single-use, and invalidated on lease change.
- [x] The first durable dispatch consumes the snapshot before the operation
      runs; an in-flight operation blocks lease acquisition.
- [x] Receipts transition `prepared -> dispatched -> applied` (or `rejected` /
      `outcome_unknown`); an uncertain dispatch is never retried; recovery on
      startup moves stranded `dispatched` rows to `outcome_unknown`.
- [x] Receipt identifier collisions with different tool/snapshot are rejected;
      `applied` receipts short-circuit to a recovered-outcome result.
- [x] Receipts are HMAC-authenticated at rest and retained indefinitely (same
      no-retry limitation as macOS v2; pruning needs a future protocol epoch).

### Observation / redaction semantics (unchanged)

- [x] Screenshot bytes and observation text travel only in the protected
      carrier (`computer-use-v2` profile with snapshot/attestation IDs, PNG
      SHA-256 + dimensions, and exact capture rectangle).
- [ ] Bounded accessibility walk: node, depth, elapsed-time, per-string,
      action-count, and total-size limits; 16 KiB observation text cap. (P2 —
      MVP ships window-level observation only, see §6.)
- [ ] Secure-text redaction: password/secure fields always redacted. (P2 with
      AT-SPI2 walk.)
- [x] Typed text, set values, and tool arguments never enter receipts or logs.

### Daemon trust boundary (Linux-adapted, see §5)

- [x] The daemon owns capture, input dispatch, leases, snapshots, and durable
      receipts; the relay owns no desktop state.
- [x] Private per-user socket: directory mode 0700, socket mode 0600.
- [x] Single-instance guarantee acquired before permissions, receipt recovery,
      or filesystem mutation.
- [x] At most 64 simultaneous connections; per-connection handshake.
- [x] Trusted MCP calls never reconnect or replay after relay failure.
- [x] Exact-window revalidation immediately before every input dispatch.

## 2. Scaffold `computer-use-linux/`

- [x] Standalone Cargo workspace (parallel to the standalone Swift package),
      pinned to the repo toolchain.
- [x] `computer-use-core` — shared protocol/runtime/models crate mirroring
      `ComputerUseCore` (AgentProtocol, Models, Geometry, ToolCatalog,
      ComputerUseRuntime, RuntimeProtocols).
- [x] `grok-computer-use-daemon` — native desktop daemon mirroring
      `GrokComputerUseApp` (socket server, peer verifier, receipt store, X11
      desktop driver).
- [x] `grok-computer-use-mcp` — stateless stdio MCP relay mirroring
      `GrokComputerUseMCP` (relay main, agent client, parent verifier).
- [x] README documenting supported environment, non-goals, and the security
      delta versus macOS.

## 3. Core port (platform-agnostic)

- [x] `JSONValue`-equivalent handling on `serde_json::Value` with finite-number
      enforcement and typed accessors.
- [x] `ComputerUseError` with the exact v2 error codes and messages.
- [x] Geometry types + `CoordinateMapper` with tests mirroring
      `GeometryTests.swift`.
- [x] Agent protocol envelope validation + HMAC session authentication with
      tests mirroring `ProtocolTests.swift`.
- [x] Tool catalog with exact v2 schemas.
- [x] Protected carrier validation (byte/dimension/SHA-256/geometry bounds).
- [x] Runtime state machine (lease/snapshot/receipt/dispatch) behind
      `DesktopDriver` + `ActionReceiptStore` + `Clock` + `IdentifierGenerator`
      traits, with fake-backed tests mirroring `RuntimeTests.swift`.

## 4. Daemon (Linux native)

- [x] Unix socket server: framed, authenticated, sequence-checked; 64-connection
      cap; per-connection session state; disconnect cleanup.
- [x] Single-instance: abstract-namespace socket bind (Linux equivalent of the
      named bootstrap port) acquired before any durable state is touched, plus
      an advisory `flock` for filesystem serialization.
- [x] Peer verification (weaker than macOS by design, documented):
      `SO_PEERCRED` same-uid enforcement, `/proc/<pid>/exe` executable
      allowlist (fixed install path), no-symlink path chain.
- [x] SQLite receipt store: `synchronous=FULL`, WAL, mode-0600 database in a
      mode-0700 directory, no-symlink open, HMAC-SHA256 row authentication
      with a mode-0600 key file (Linux stand-in for Keychain — documented
      delta), `prepared/dispatched/applied/rejected/outcome_unknown`
      transitions, startup recovery of stranded dispatches.
- [x] X11 desktop driver:
  - [x] Window enumeration via EWMH (`_NET_CLIENT_LIST`, `_NET_WM_PID`,
        `WM_CLASS`, `_NET_ACTIVE_WINDOW`).
  - [x] `bundle_id` mapping: Linux uses the WM_CLASS class name (documented).
  - [x] Exact-window capture via `GetImage` with exact global geometry
        (translated to root coordinates) and bounded PNG encode (downscale to
        the 1,280/900 KB contract with recorded final dimensions).
  - [x] Exact-window revalidation (same window ID, same PID, same geometry)
        immediately before every input dispatch; fail closed on mismatch.
  - [x] XTest mouse: click (left/right, single/double), drag, smooth scroll
        (button 4–7 mapping from pages).
  - [x] XTest keyboard: named keys + modifier chords (`ctrl`, `alt`, `shift`,
        `super` accepted; macOS `command`→`super`, `option`→`alt` aliases);
        text entry via keyboard-mapping lookup with a temporary spare-keycode
        remap for unmapped symbols.
  - [ ] (P2) AT-SPI2: bounded accessibility tree walk, element targets,
        `perform_secondary_action`, `set_value`, focused-editable check before
        text entry, secure-text redaction. MVP behavior: `get_app_state`
        returns window-level observation with no elements; element-addressed
        tools return `invalid_arguments`.
- [x] Daemon main: instance lock → receipt recovery → socket server; clean
      shutdown.

## 5. Relay + integration

- [x] Stdio MCP relay: JSON-RPC 2.0 `initialize`/`tools/list`/`tools/call`,
      v2 catalog served verbatim, per-launch single connection to the daemon
      (no reconnect, no replay), action identifiers minted per call,
      protected carrier passed through untouched.
- [x] Parent-process verification: direct parent must be a same-uid allowlisted
      Grok host executable (documented as weaker than macOS team checks).
- [ ] Rust host wiring: extend
      `crates/codegen/xai-grok-shell/src/util/computer_use.rs` with a Linux
      `trusted_relay_path()` (fixed install path
      `~/.local/libexec/grok-computer-use/grok-computer-use-mcp`, no-symlink
      chain, same-uid ownership, X11 session check) so
      `grok computer-use status|enable` works on Ubuntu/X11. **Requires a
      decision on relaxing the macOS-only signature gate; see README §Security
      delta.**
- [ ] CI: Linux job building `computer-use-linux/`, running core/daemon unit
      tests headless, plus an Xvfb end-to-end fixture (capture → attest →
      click → receipt applied).
- [x] Install script for Ubuntu (fixed paths, systemd user service optional)
      and uninstall.

## 6. MVP reductions (decision checkpoint outcomes)

Per the approved plan, if AT-SPI2 semantic targeting is too weak the MVP ships
as "window + screenshot + mouse/keyboard + focused text entry". That reduction
is **taken** for the first cut:

- `get_app_state` returns geometry + screenshot + window title; the
  accessibility tree string is a one-line window summary and `elements` is
  empty.
- `click`/`drag`/`scroll` work in pixel space (scroll falls back to wheel
  events at a pixel point when no element is available — schema extended with
  an optional pixel target **only if** the reserved Rust catalog validation
  permits; otherwise scroll stays element-only and is unavailable until P2).
- `perform_secondary_action` and `set_value` return `invalid_arguments`
  ("no accessibility elements in this snapshot") until AT-SPI2 lands.
- `type_text` targets the focused window after revalidation (AT-SPI2
  focused-editable precondition is P2).

## 7. Definition of done (MVP)

- [ ] On Ubuntu X11, `grok computer-use enable` succeeds against the Linux
      relay. (blocked on §5 Rust host wiring)
- [x] Trusted tools can capture, observe, click, type, and scroll on a target
      window (daemon + relay implemented; manual smoke test verified the full
      relay→daemon→X11 path: handshake, tools/list, list_apps, and an exact
      `get_app_state` capture of a live X11 window with a valid protected
      carrier; input tools are unit-tested, with the automated Xvfb fixture
      pending CI).
- [x] Snapshot binding, lease serialization, receipts, and replay protection
      behave identically to macOS at the protocol level (shared-core tests).
- [ ] Linux CI validates protocol/runtime tests plus an Xvfb end-to-end
      fixture.
- [x] README states the supported environment, non-goals, and macOS security
      deltas.
