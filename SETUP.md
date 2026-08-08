# Local macOS setup

This runs Grok Build in the terminal with the background **Grok Computer Use**
macOS companion. Local mode supports macOS 14 or newer on Intel and Apple
Silicon.

## 1. Install prerequisites

Install:

- Xcode Command Line Tools
- Rust (the repository selects the required toolchain)
- Protocol Buffers (`protoc`)

## 2. Build Grok

From the repository root:

```sh
PROTOC="$(command -v protoc)" cargo build -p xai-grok-pager-bin
```

## 3. Install the computer-use companion

For permissions that survive rebuilds, first select a stable Apple Development
identity from:

```sh
security find-identity -v -p codesigning
export GROK_COMPUTER_USE_CODESIGN_IDENTITY="Apple Development: Your Name (TEAMID)"
```

Use the same exported identity whenever building the companion or launching
Grok. If no identity is configured, local mode falls back to ad-hoc signing.

```sh
export GROK_COMPUTER_USE_LOCAL_DEV=1
computer-use-macos/scripts/install-app.sh --build
```

Grant **Grok Computer Use** access in:

- System Settings → Privacy & Security → Accessibility
- System Settings → Privacy & Security → Screen Recording

Restart the companion after granting access:

```sh
pkill -x GrokComputerUseApp || true
open -gj "$HOME/Applications/Grok Computer Use.app"
```

The companion runs in the background and does not have a normal app window.

## 4. Enable computer use once

```sh
computer-use-macos/scripts/run-local-grok.sh computer-use enable
```

This saves the computer-use setting and exits. It does not open the TUI.

## 5. Run Grok Build

```sh
computer-use-macos/scripts/run-local-grok.sh
```

Sign in when prompted. After the one-time enablement, this is the only command
needed for normal use.

## After rebuilding the companion

When using a stable Apple Development identity, rebuilding preserves the app's
macOS permission identity. Keep the certificate, bundle identifier, and install
path unchanged.

When using the default ad-hoc signature, rebuilding changes the permission
identity. Reset and grant both permissions again:

```sh
tccutil reset Accessibility com.xai.grok.computer-use
tccutil reset ScreenCapture com.xai.grok.computer-use
open -gj "$HOME/Applications/Grok Computer Use.app"
```

Do not rebuild the companion between granting permissions and testing it.
