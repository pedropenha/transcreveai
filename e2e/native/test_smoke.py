"""Native smoke tests for the Windows E2E harness (T-009).

Deliberately minimal: these prove the harness can launch a test build, attach
through UI Automation, and drive the single-instance CLI channel. Real
dictation flows (press global shortcut → WAV fixture plays → text inserted at
cursor) stay on the manual smoke checklist in README.md until T-032 builds the
app matrix on top of this harness.
"""

import pytest

from conftest import hub_window, send_cli

pytestmark = pytest.mark.smoke


def test_app_process_attaches(app):
    """The test build launches and stays alive long enough to attach via UIA."""
    assert app.process is not None


def test_hub_window_exists(app):
    """The main webview window ('hub', title "Transcreve.ai") is created, even
    when the app started minimized to the tray."""
    win = hub_window(app)
    assert win.exists()


def test_cli_remote_control_channel(app, app_exe):
    """A second instance forwards `--cancel` through the single-instance
    channel and exits cleanly. `--cancel` is the safe flag: it never injects
    text or touches the foreground app."""
    result = send_cli("--cancel", exe=app_exe)
    assert result.returncode == 0, result.stderr
