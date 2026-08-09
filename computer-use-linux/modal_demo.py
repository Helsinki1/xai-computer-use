from __future__ import annotations

import os
import socket
import subprocess
import time
from pathlib import Path

import modal


APP_NAME = "linux-computer-use-mvp"
LOCAL_SUBTREE = Path(__file__).resolve().parent
LOCAL_REPO_ROOT = LOCAL_SUBTREE.parent
REMOTE_REPO_ROOT = "/root/xai-computer-use"
REMOTE_SUBTREE = f"{REMOTE_REPO_ROOT}/computer-use-linux"
SUPPORT_DIR = "/root/.local/share/grok-computer-use"
RUNTIME_DIR = "/root/modal-runtime"
LOG_DIR = "/root/modal-logs"


desktop_logs = modal.Volume.from_name(f"{APP_NAME}-logs", create_if_missing=True)
xai_secret = modal.Secret.from_name("xai-api-key", required_keys=["XAI_API_KEY"])

image = (
    modal.Image.debian_slim(python_version="3.11")
    .apt_install(
        "build-essential",
        "ca-certificates",
        "chromium",
        "curl",
        "novnc",
        "openbox",
        "pkg-config",
        "protobuf-compiler",
        "python3",
        "python3-xdg",
        "websockify",
        "x11-utils",
        "x11vnc",
        "xauth",
        "xterm",
        "xvfb",
    )
    .env(
        {
            "CARGO_HOME": "/root/.cargo",
            "RUSTUP_HOME": "/root/.rustup",
            "PATH": "/root/.cargo/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin",
        }
    )
    .add_local_dir(str(LOCAL_REPO_ROOT), REMOTE_REPO_ROOT, copy=True)
    .run_commands(
        "curl https://sh.rustup.rs -sSf | sh -s -- -y --profile minimal --default-toolchain 1.94.0",
        f"bash -lc 'source /root/.cargo/env && cd {REMOTE_REPO_ROOT} && cargo build --release -p xai-grok-pager-bin'",
        f"bash -lc 'source /root/.cargo/env && cd {REMOTE_SUBTREE} && cargo build --release'",
        f"bash -lc 'cd {REMOTE_SUBTREE} && ./scripts/install.sh'",
        f"chmod +x {REMOTE_SUBTREE}/scripts/start_modal_x11_demo.sh {REMOTE_SUBTREE}/scripts/start_modal_grok.sh {REMOTE_SUBTREE}/scripts/mcp_smoke_test.py {REMOTE_SUBTREE}/scripts/modal_restaurant_demo.py",
    )
)

app = modal.App(APP_NAME)


def desktop_env() -> dict[str, str]:
    return {
        **os.environ,
        "HOME": "/root",
        "DISPLAY_NUM": ":1",
        "SCREEN_GEOMETRY": "1440x900x24",
        "XDG_RUNTIME_DIR": RUNTIME_DIR,
        "LOG_ROOT": LOG_DIR,
    }


def wait_for_port(
    port: int,
    *,
    timeout_seconds: float,
    process: subprocess.Popen[bytes] | None = None,
) -> None:
    deadline = time.time() + timeout_seconds
    while time.time() < deadline:
        if process is not None and process.poll() is not None:
            raise RuntimeError(f"desktop process exited with status {process.returncode}")
        try:
            with socket.create_connection(("127.0.0.1", port), timeout=1):
                return
        except OSError:
            time.sleep(0.5)
    raise RuntimeError(f"timed out waiting for port {port}")


@app.server(
    image=image,
    port=6080,
    unauthenticated=True,
    min_containers=1,
    scaledown_window=60 * 30,
    volumes={LOG_DIR: desktop_logs},
    secrets=[xai_secret],
)
class Desktop:
    @modal.enter()
    def start(self) -> None:
        self.process = subprocess.Popen(
            ["/bin/bash", "-lc", f"cd {REMOTE_SUBTREE} && ./scripts/start_modal_x11_demo.sh"],
            env=desktop_env(),
            start_new_session=True,
        )
        wait_for_port(6080, timeout_seconds=180, process=self.process)

    @modal.exit()
    def stop(self) -> None:
        if getattr(self, "process", None) is None or self.process.poll() is not None:
            return
        self.process.terminate()
        try:
            self.process.wait(timeout=10)
        except subprocess.TimeoutExpired:
            self.process.kill()


@app.function(
    image=image,
    timeout=60 * 10,
    volumes={LOG_DIR: desktop_logs},
    secrets=[xai_secret],
)
def smoke_test() -> str:
    env = {
        **desktop_env(),
        "DISPLAY": ":1",
        "XDG_SESSION_TYPE": "x11",
    }
    subprocess.run(["mkdir", "-p", env["XDG_RUNTIME_DIR"], LOG_DIR], check=True)
    xvfb = subprocess.Popen(
        ["Xvfb", ":1", "-screen", "0", "1440x900x24", "-ac", "+extension", "RANDR"],
        env=env,
    )
    daemon = None
    try:
        daemon = subprocess.Popen(
            ["/root/.local/libexec/grok-computer-use/grok-computer-use-daemon"],
            env=env,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )
        socket_path = Path(env["XDG_RUNTIME_DIR"]) / "grok-computer-use" / "agent-v2.sock"
        for _ in range(30):
            if socket_path.exists():
                break
            time.sleep(1)
        if not socket_path.exists():
            raise RuntimeError("daemon socket did not become ready")
        completed = subprocess.run(
            ["python3", f"{REMOTE_SUBTREE}/scripts/mcp_smoke_test.py"],
            env=env,
            check=True,
            capture_output=True,
            text=True,
        )
        return completed.stdout
    finally:
        if daemon is not None:
            daemon.terminate()
            try:
                daemon.wait(timeout=5)
            except subprocess.TimeoutExpired:
                daemon.kill()
        xvfb.terminate()
        xvfb.wait(timeout=5)


@app.function(
    image=image,
    timeout=60 * 2,
    volumes={LOG_DIR: desktop_logs},
)
def tail_logs() -> str:
    lines: list[str] = []
    for name in ["browser.log", "daemon.log", "demo.log", "openbox.log", "x11vnc.log", "xvfb.log"]:
        path = Path(LOG_DIR) / name
        if path.exists():
            lines.append(f"== {name} ==")
            lines.extend(path.read_text().splitlines()[-40:])
    return "\n".join(lines)


@app.local_entrypoint()
def main() -> None:
    print("Reusable deployment:")
    print("  modal deploy computer-use-linux/modal_demo.py")
    print("")
    print("Then open the deployed web endpoint for the `desktop` function.")
    print("")
    print("One-off checks:")
    print("  modal run computer-use-linux/modal_demo.py::smoke_test")
    print("  modal run computer-use-linux/modal_demo.py::tail_logs")
