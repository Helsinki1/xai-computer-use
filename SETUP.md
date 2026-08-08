# Local macOS setup

This runs Grok Build in the terminal with the background **Grok Computer Use**
companion. It supports macOS 14 or newer on Intel and Apple Silicon.

## 1. Install prerequisites

Install Xcode Command Line Tools, Rust, and Protocol Buffers (`protoc`).

## 2. Choose a stable signing identity

A stable Apple Development identity lets macOS retain Accessibility and Screen
Recording permissions across rebuilds:

```sh
security find-identity -v -p codesigning
export GROK_COMPUTER_USE_CODESIGN_IDENTITY="<Apple Development identity or SHA-1>"
```

Use the same identity in each terminal where you launch local Grok. Without
one, the scripts use ad-hoc signing and macOS may require permissions again
after a rebuild.

## 3. Enable and launch

From the repository root, enable computer use once:

```sh
computer-use-macos/scripts/run-local.sh computer-use enable
```

Then launch Grok Build:

```sh
computer-use-macos/scripts/run-local.sh
```

`run-local.sh` prompts for `XAI_API_KEY` when it is not already exported. It
also builds Grok, installs or updates the companion when needed, signs both
components consistently, and launches the TUI.

Approve **Grok Computer Use** under System Settings → Privacy & Security →
Accessibility and Screen Recording when macOS requests access. If permissions
changed while the app was running, restart it and rerun the command:

```sh
pkill -x GrokComputerUseApp || true
open "$HOME/Applications/Grok Computer Use.app"
computer-use-macos/scripts/run-local.sh --no-rebuild
```

## Rebuild or reuse the current app

Force a fresh rebuild and replacement:

```sh
computer-use-macos/scripts/run-local.sh --rebuild
```

Launch without rebuilding anything:

```sh
computer-use-macos/scripts/run-local.sh --no-rebuild
```

Normal launches need only `computer-use-macos/scripts/run-local.sh`; unchanged
Swift and Rust targets use their incremental builds.
