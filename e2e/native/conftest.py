"""Shared fixtures for the Windows-native E2E harness (T-009).

These tests drive a real `transcreve-ai.exe` through UI Automation via
pywinauto. They are intentionally a *skeleton* for the manual smoke checklist —
native flows need a Windows desktop session, a build compiled with
`--features audio-fixture`, and (for dictation) the WAV fixture env var.

Configuration via environment variables:

- `TRANSCREVE_E2E_EXE` — path to the test binary. Default:
  `<CARGO_TARGET_DIR>\debug\transcreve-ai.exe`.
- `TRANSCREVE_AUDIO_FIXTURE` — WAV fed instead of the microphone. Default:
  `e2e/fixtures/pt-br/fala_curta.wav` (resolved relative to the repo root).
"""

import os
import subprocess
import sys
import time
from pathlib import Path

import pytest

REPO_ROOT = Path(__file__).resolve().parents[2]
DEFAULT_EXE = (
    Path(os.environ.get("CARGO_TARGET_DIR", REPO_ROOT / "src-tauri" / "target"))
    / "debug"
    / "transcreve-ai.exe"
)
DEFAULT_FIXTURE = REPO_ROOT / "e2e" / "fixtures" / "pt-br" / "fala_curta.wav"
HUB_WINDOW_TITLE = "Transcreve.ai"


def _exe_path() -> Path:
    return Path(os.environ.get("TRANSCREVE_E2E_EXE", DEFAULT_EXE))


def _fixture_wav() -> Path:
    return Path(os.environ.get("TRANSCREVE_AUDIO_FIXTURE", DEFAULT_FIXTURE))


@pytest.fixture(scope="session")
def app_exe() -> Path:
    exe = _exe_path()
    if not exe.is_file():
        pytest.skip(
            f"test build not found at {exe} — build with "
            "`cargo build --features audio-fixture` or set TRANSCREVE_E2E_EXE"
        )
    return exe


@pytest.fixture(scope="session")
def app(app_exe):
    """Launch the app (single instance, test build) and attach via UIA."""
    pywinauto = pytest.importorskip("pywinauto")

    env = dict(os.environ)
    env["TRANSCREVE_AUDIO_FIXTURE"] = str(_fixture_wav())

    proc = subprocess.Popen([str(app_exe)], env=env)
    try:
        application = pywinauto.Application(backend="uia").connect(
            process=proc.pid, timeout=30
        )
        yield application
    finally:
        proc.terminate()
        try:
            proc.wait(timeout=10)
        except subprocess.TimeoutExpired:
            proc.kill()


def hub_window(application, timeout: float = 15.0):
    """The main ('hub') window. Hidden windows still enumerate under UIA, so
    this resolves even when the app started minimized to the tray."""
    deadline = time.monotonic() + timeout
    while True:
        win = application.window(title_re=f".*{HUB_WINDOW_TITLE}.*")
        if win.exists(timeout=0.5):
            return win
        if time.monotonic() > deadline:
            raise TimeoutError(f"hub window '{HUB_WINDOW_TITLE}' never appeared")


def send_cli(*args: str, exe: Path | None = None) -> subprocess.CompletedProcess:
    """Remote-control the running instance via a second invocation
    (`--toggle-transcription`, `--cancel`, …), which exits immediately after
    forwarding the command through the single-instance channel."""
    exe = exe or _exe_path()
    return subprocess.run(
        [str(exe), *args], capture_output=True, text=True, timeout=15
    )


# Keep a Windows-only guard at collection time: pywinauto's UIA backend is
# meaningless elsewhere.
collect_ignore_glob = [] if sys.platform == "win32" else ["*.py"]
