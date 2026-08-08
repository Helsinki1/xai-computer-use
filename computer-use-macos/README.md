# Grok Computer Use for macOS

This subtree contains the native macOS 14/Apple Silicon computer-use app and its stateless stdio MCP relay. The installed path is fixed at `~/Applications/Grok Computer Use.app`; Grok refuses user-configured substitutes.

## Build and install

1. Set `GROK_COMPUTER_USE_CODESIGN_IDENTITY` to the stable Developer ID Application identity, `GROK_COMPUTER_USE_EXPECTED_TEAM_ID` to Grok's release team, and `GROK_COMPUTER_USE_NOTARY_PROFILE` to an `xcrun notarytool store-credentials` keychain profile.
2. Run `computer-use-macos/scripts/certify.sh` on Apple Silicon macOS 14 or newer. This builds, signs, notarizes, staples, and verifies the exact release bundle.
3. Run `computer-use-macos/scripts/install-app.sh` without `--build`, so installation uses the certified bundle instead of replacing it with a fresh unnotarized build.
4. Approve Accessibility and Screen Recording for **Grok Computer Use** in System Settings. Restart the app after changing either permission.
5. Run `grok computer-use status`, then `grok computer-use enable`. Start a new Grok session.

Ad-hoc signing (`GROK_COMPUTER_USE_CODESIGN_IDENTITY=-`) is supported only for local build/test. The trusted Grok profile also performs Gatekeeper assessment, so production builds must be signed consistently and notarized. Reusing the same bundle identifier and signing identity preserves the app's TCC identity across upgrades.

## Local debug mode

Intel Macs and machines without a Developer ID certificate can run an explicit
debug-only mode. Release builds do not contain the ad-hoc trust branch.

Set `GROK_COMPUTER_USE_CODESIGN_IDENTITY` to a stable Apple Development
identity to preserve macOS privacy permissions across local rebuilds. Without
one, the scripts fall back to ad-hoc signing and permissions must be granted
again after every rebuild.

```sh
computer-use-macos/scripts/run-local.sh computer-use enable
computer-use-macos/scripts/run-local.sh
```

`run-local.sh` prompts for `XAI_API_KEY` when it is not already exported,
installs a debug app only when it is missing or stale, uses Cargo's incremental
build, and then launches Grok. Pass `--rebuild` to reinstall the app
unconditionally, or `--no-rebuild` to launch only the existing artifacts.
Arguments after `--` are passed to Grok.

Approve Accessibility and Screen Recording for **Grok Computer Use** when
macOS prompts, then restart both the app and Grok. Local mode still requires
the fixed install path, exact app and relay identifiers, valid ad-hoc code
seals, the allowlisted Grok executable identity, private authenticated socket,
and the complete snapshot/action safety protocol. It skips only the production
Apple Silicon, Team ID, Gatekeeper, and notarization gates.

The installer stages and verifies the new bundle before moving it into place. An existing install is moved to a timestamped backup and is never silently deleted.

## Coordinate contract

Every screenshot is bound to its exact final PNG dimensions and the ScreenCaptureKit window rectangle in Quartz global points. Inputs use continuous PNG edge-space:

```text
0 <= x < png_width, 0 <= y < png_height
global_x = capture_origin_x + x * capture_width_points / png_width
global_y = capture_origin_y + y * capture_height_points / png_height
```

There is no half-pixel offset, clamp, rounding, Y flip, backing-scale inference, or identity fallback. Distinct Swift types keep PNG coordinates separate from global points. The encoder produces an sRGB RGBA8 PNG, at most 900,000 bytes, 1,280 pixels per side, and 1,638,400 pixels total; capture fails closed if it cannot satisfy that contract.

## Security and concurrency

- The signed LaunchServices app owns ScreenCaptureKit, Accessibility, input dispatch, leases, snapshots, and receipts. The relay owns no desktop state.
- The Unix socket lives in a mode-0700 user directory and is mode 0600. Both peers bind running-code verification to the socket's immutable audit token, then require the exact executable path, Apple-anchored signature, team, identifier, and sealed app bundle. Before connecting, the relay also requires its direct parent to be a valid, same-team, allowlisted Grok host; launching the signed relay from another same-user process fails closed.
- A named bootstrap port is acquired before permissions, receipt recovery, or any filesystem state, and is retained by every accepted connection until in-flight work completes. A mode-0600 `flock` remains as filesystem serialization; it is not the process-singleton trust root. The server admits at most 64 simultaneous connections.
- Every connection performs a fresh HMAC-authenticated session handshake. Monotonic sequence numbers reject replay and cross-connection messages.
- A fenced desktop lease serializes workflows. Snapshots are server-authoritative, delivery-attested, session-bound, and single-use. The first durable dispatch consumes the snapshot before the runtime's first suspension point.
- Action receipts use a no-symlink, private SQLite database and validated WAL/SHM sidecars with `synchronous=FULL`, `fullfsync=ON`, and Keychain-backed HMAC-SHA256 authentication. State is `prepared -> dispatched -> applied|rejected`; a stranded or uncertain dispatch becomes `outcome_unknown` and is never retried.
- Screenshot bytes and bounded AX observation text exist only in the reserved protected MCP carrier. Rust verifies and removes that carrier before ordinary tool output, logs, hooks, replay, or caches see it. Text-entry values and tool arguments are never written to receipts or logs.

The AX walk has node, depth, elapsed-time, per-string, action-count, and total-observation limits. ScreenCaptureKit operations have five-second deadlines. Every input revalidates the exact captured window; text entry additionally requires an editable focused AX element. Secure text values are always redacted.

## Current operational limit

Version 2 retains authenticated action receipts indefinitely so an old action identifier can never become executable again. Safe bounded pruning requires a future protocol epoch or authenticated high-water mark; deleting terminal rows by age would break the no-retry guarantee. Long-lived, high-volume test fleets must monitor the receipt database until that protocol extension lands.

## Verification

Run `computer-use-macos/scripts/certify.sh` on an Apple Silicon macOS runner. It validates plists and scripts, runs fake-backed Swift tests, builds both executables, assembles/signs the app, submits that exact bundle for notarization, staples the ticket, and verifies the code seal and Gatekeeper acceptance. Set `GROK_COMPUTER_USE_RUN_RUST_CONTRACT_TESTS=1` to also run the Rust carrier contract tests.

The repository-level `.github/workflows/computer-use-macos.yml` defines the macOS CI job.

Linux cannot compile or execute AppKit, Accessibility, ScreenCaptureKit, Security, Keychain, or macOS SQLite/fullfsync behavior. On Linux, only source/contract inspection and shell/YAML checks are meaningful; release certification must run on the macOS target above.
