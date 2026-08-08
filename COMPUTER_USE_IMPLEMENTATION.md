# Native computer-use MCP implementation

## What was built

This change adds a reserved external MCP capability for native computer use on
Apple Silicon Macs running macOS 14 or newer. The capability is disabled by
default and is enabled with `grok computer-use enable` after Grok verifies the
fixed app location, code signatures, team identity, and Gatekeeper status.

The implementation has four trust boundaries:

1. A signed LaunchServices app owns ScreenCaptureKit, Accessibility, input
   dispatch, desktop leases, snapshot state, and durable action receipts.
2. A stateless signed stdio MCP relay connects Grok to that app over a private
   authenticated Unix socket.
3. The Rust MCP layer reserves `xai_computer_use`, validates the exact v2 tool
   catalog and observation envelope, and removes screenshots from ordinary MCP
   output.
4. The sampler adds the screenshot only to the final inference request, proves
   the exact PNG occurs once in the serialized request body, and acknowledges
   delivery before the snapshot can authorize an action.

Generic MCP configuration, MCP JSON imports, meta-dispatch tools, workspace
proxies, and remote campaigns cannot mint or invoke the trusted capability.

## Pixel coordinate contract

The native app captures one exact window and records both its Quartz global
rectangle in points and the dimensions of the final bounded PNG. Model
coordinates are continuous PNG edge coordinates with a top-left origin:

```text
0 <= x < png_width
0 <= y < png_height

global_x = capture_origin_x + x * capture_width_points / png_width
global_y = capture_origin_y + y * capture_height_points / png_height
```

X and Y use independent ratios. There is no rounding, clamping, half-pixel
offset, Y-axis flip, inferred Retina scale, or identity fallback. Before input
dispatch, the app revalidates the target process, bundle, window identifier,
and exact window bounds. Element actions prefer Accessibility semantics; raw
pixel coordinates are the fallback.

## Correctness, durability, and concurrency

- A fixed-name bootstrap port is acquired before permissions, receipt
  recovery, or filesystem mutation, preventing a second app instance from
  changing the live instance's durable state.
- A fenced desktop lease serializes workflows on each Mac. Heartbeats retain
  the lease while protected inference is running.
- Snapshots are server-generated, session-bound, delivery-attested,
  single-use, and tied to the exact PNG hash and dimensions.
- A trusted tool must be the only direct call in its model turn. Protected
  responses allow no call, one observation, or one effectful action tied to
  the exact snapshot. Mixed, tunneled, malformed, and multi-call responses
  fail closed.
- Trusted MCP calls never reconnect or replay. This prevents a relay path that
  changed after the shell's one-time signature verification from inheriting
  the in-process capability.
- SQLite receipts transition through `prepared -> dispatched -> applied` (or
  `rejected` / `outcome_unknown`) with full sync and Keychain-backed HMAC
  authentication. An uncertain dispatch is never retried.
- IPC frames are capped at 2 MiB and the native server admits at most 64
  simultaneous connections.
- Screenshot bytes, Accessibility text, typed values, and trusted arguments do
  not enter ordinary chat history, hooks, replay buffers, logs, or disk caches.

Large test campaigns scale across Mac workers. Each worker intentionally has
one serialized desktop lease; orchestration should shard independent tests
across independently provisioned hosts rather than race one interactive
desktop.

## Operator workflow

```text
computer-use-macos/scripts/install-app.sh
grok computer-use status
grok computer-use enable
```

Production release certification requires a stable Developer ID identity, an
expected team identifier, and an `notarytool` keychain profile. The
certification flow signs, notarizes, staples, and verifies the exact app bundle.
The root workflow performs fake-backed Swift tests, Rust contract tests, app
assembly, and ad-hoc seal checks on an Apple Silicon macOS runner.

## Files added

### Native macOS app and MCP relay

- `computer-use-macos/Package.swift`
- `computer-use-macos/README.md`
- `computer-use-macos/Resources/GrokComputerUse.entitlements`
- `computer-use-macos/Resources/Info.plist`
- `computer-use-macos/Sources/ComputerUseCore/AgentProtocol.swift`
- `computer-use-macos/Sources/ComputerUseCore/ComputerUseRuntime.swift`
- `computer-use-macos/Sources/ComputerUseCore/Geometry.swift`
- `computer-use-macos/Sources/ComputerUseCore/HostSigningPolicy.swift`
- `computer-use-macos/Sources/ComputerUseCore/MCPServer.swift`
- `computer-use-macos/Sources/ComputerUseCore/Models.swift`
- `computer-use-macos/Sources/ComputerUseCore/RuntimeProtocols.swift`
- `computer-use-macos/Sources/ComputerUseCore/ToolCatalog.swift`
- `computer-use-macos/Sources/ComputerUseCore/TransportRecoveryPolicy.swift`
- `computer-use-macos/Sources/GrokComputerUseApp/AgentSocketServer.swift`
- `computer-use-macos/Sources/GrokComputerUseApp/AppMain.swift`
- `computer-use-macos/Sources/GrokComputerUseApp/DurableReceiptStore.swift`
- `computer-use-macos/Sources/GrokComputerUseApp/MacOSDesktopDriver.swift`
- `computer-use-macos/Sources/GrokComputerUseApp/PeerVerifier.swift`
- `computer-use-macos/Sources/GrokComputerUseMCP/AgentClient.swift`
- `computer-use-macos/Sources/GrokComputerUseMCP/ParentProcessVerifier.swift`
- `computer-use-macos/Sources/GrokComputerUseMCP/RelayMain.swift`
- `computer-use-macos/Tests/ComputerUseCoreTests/GeometryTests.swift`
- `computer-use-macos/Tests/ComputerUseCoreTests/HostSigningPolicyTests.swift`
- `computer-use-macos/Tests/ComputerUseCoreTests/ProtocolTests.swift`
- `computer-use-macos/Tests/ComputerUseCoreTests/RuntimeTests.swift`
- `computer-use-macos/scripts/build-app.sh`
- `computer-use-macos/scripts/certify.sh`
- `computer-use-macos/scripts/install-app.sh`
- `computer-use-macos/scripts/notarize-app.sh`
- `computer-use-macos/scripts/verify-bundle.sh`
- `computer-use-macos/scripts/verify-install.sh`

### Rust integration and CI

- `.github/workflows/computer-use-macos.yml`
- `crates/codegen/xai-grok-mcp/src/computer_use.rs`
- `crates/codegen/xai-grok-pager/src/computer_use_cmd.rs`
- `crates/codegen/xai-grok-sampler/src/protected_overlay.rs`
- `crates/codegen/xai-grok-sampler/tests/protected_overlay_wire.rs`
- `crates/codegen/xai-grok-shell/src/util/computer_use.rs`

## Existing files changed

- Dependency wiring: `Cargo.lock`, `xai-grok-mcp/Cargo.toml`, and
  `xai-grok-sampler/Cargo.toml` add bounded PNG/hash validation dependencies.
- MCP host: `xai-grok-mcp/src/lib.rs` and `servers.rs` add the reserved trusted
  profile, exact catalog validation, hidden lifecycle calls, observation
  carrier, no-replay behavior, and protected output capture.
- Sampler: `actor/mod.rs`, `actor/request_task.rs`, `client.rs`, `commands.rs`,
  `handle.rs`, and `lib.rs` add move-only protected overlays, final-body
  attestation, backend coverage, and retry suppression.
- Shell session: `extensions/mcp.rs`, `session/acp_session.rs`, the
  `acp_session_impl` run-loop/MCP/sampler/tool-call/tool-dispatch/turn modules,
  `compaction.rs`, `managed_mcp.rs`, and `mcp_servers.rs` add trusted session
  state, redaction, handoff ordering, heartbeats, invalidation, and exact-call
  dispatch gates.
- Shell configuration: `util/config/mcp.rs` rejects the reserved name from all
  generic import paths and stores the separate local `computer_use.enabled`
  policy; `util/mod.rs` exports platform verification helpers.
- Tool runtime: `xai-grok-tools/src/registry/types.rs` and
  `xai-grok-workspace/src/workspace_ops.rs` add invocation-scoped typed context
  for local dispatch and reject capability transport through workspace proxies.
- CLI: `xai-grok-pager/src/app/cli.rs`, `xai-grok-pager/src/lib.rs`, and
  `xai-grok-pager-bin/src/main.rs` expose `computer-use status|enable|disable`.
- Session test fixtures were updated for the new lease/stream-tracker state in
  `cancel_running_task_tests.rs`, `client_hooks_tests.rs`,
  `idle_resume_tests.rs`, `inline_auto_compact_flow_tests.rs`,
  `memory_config_tests.rs`, `plan_approval_resume_tests.rs`,
  `plan_mode_edit_gate_tests.rs`, `replay_buffer_send_update_tests.rs`,
  `support.rs`, and `tool_layer_images_bridge_tests.rs`.

## Verification and known limits

The contract suite covers MCP observation validation, protected sampler bodies
for Chat Completions/Responses/Messages, no-retry behavior, reserved-name
forgery attempts, mixed-call rejection, snapshot binding, redaction, and
session invalidation. Native tests cover geometry, protocol authentication,
signing policy, receipt transitions, leases, and runtime behavior.

Two release boundaries remain explicit:

1. Swift/AppKit/Accessibility/ScreenCaptureKit/TCC, Developer ID signing,
   notarization, Gatekeeper, and live Retina/multi-display coordinates require
   the provisioned Apple Silicon macOS certification job.
2. Authenticated receipts are retained indefinitely. Safe bounded pruning needs
   an authenticated protocol epoch or high-water mark; deleting rows by age
   would violate the no-retry guarantee. High-volume fleets must monitor the
   receipt database until that extension is implemented.

