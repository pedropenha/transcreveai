# Windows-native E2E harness (`windows-desktop-e2e`)

Drives a real `transcreve-ai.exe` through UI Automation via
[pywinauto](https://pywinauto.readthedocs.io/). This is the base for native
flows the Playwright layer cannot reach: global low-level shortcuts, the tray,
the Flow Bar overlay, clipboard/paste into other apps.

> Scope for T-009: a working skeleton plus smoke tests. The real dictation
> flows are a **manual smoke checklist** (below); T-032 automates the app
> matrix on top of this harness.

## Prerequisites

1. Windows desktop session (UIA needs an interactive session — not SSH/headless
   for window automation).
2. `pip install -r e2e/native/requirements.txt`
3. A test build with the WAV audio source:

   ```powershell
   . .\scripts\windows-dev-env.ps1 -BypassJunction -TargetDir C:\t-<lane>
   cargo build --features audio-fixture
   ```

4. Point the harness at the binary and a fixture WAV:

   ```powershell
   $env:TRANSCREVE_E2E_EXE = "C:\t-<lane>\debug\transcreve-ai.exe"
   $env:TRANSCREVE_AUDIO_FIXTURE = "$PWD\e2e\fixtures\pt-br\fala_curta.wav"
   ```

   Both default sensibly (`<CARGO_TARGET_DIR>\debug\transcreve-ai.exe` and
   `e2e/fixtures/pt-br/fala_curta.wav`) when unset.

## Running

```powershell
python -m pytest e2e/native -m smoke -v
```

Tests skip cleanly when the binary does not exist.

## Manual smoke checklist (what this harness will automate)

1. Launch the test build with `TRANSCREVE_AUDIO_FIXTURE=fala_curta.wav`.
2. Focus Notepad, hold the dictation shortcut (`Ctrl+Win`) for ~2 s, release →
   the fixture's first ~2 s are captured, transcribed, and the text lands in
   Notepad. (With the current synthetic fixtures, any transcript is acceptable —
   the assertion is on the _flow_, not the words.)
3. Repeat with `fala_longa_muletas.wav` in hands-free mode for the full ~20 s.
4. `transcreve-ai.exe --toggle-transcription` starts a session;
   `--cancel` aborts it without inserting text.
5. No-microphone machine: with the fixture set, the app must still open the
   "stream" and record.

## Notes

- The hub window title is `Transcreve.ai`; pywinauto finds it via UIA even when
  minimized to tray (hidden windows still enumerate).
- The app is single-instance: the harness launches it once per session and the
  CLI flags (`send_cli` in `conftest.py`) remote-control that instance.
- UIA flakiness: prefer `print_control_identifiers()` on the hub window when a
  lookup fails; add waits rather than sleeps where possible.
