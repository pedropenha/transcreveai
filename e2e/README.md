# E2E test base (T-009)

Two complementary layers, per `rules/common/testing.md` and the F002 technical
notes:

| Layer   | Tooling                                           | Location                 | What it covers                                                                                                                     |
| ------- | ------------------------------------------------- | ------------------------ | ---------------------------------------------------------------------------------------------------------------------------------- |
| Webview | Playwright (`e2e-testing`)                        | [`tests/`](../tests)     | React pages/components of the `hub` (and later `flowbar`) webviews, served by `vite dev` with the Tauri IPC surface mocked in-page |
| Native  | `windows-desktop-e2e` (pywinauto / UI Automation) | [`e2e/native/`](native/) | OS-level flows: window/tray presence, global shortcuts, dictation session against a real build                                     |

Both rely on the same building blocks:

## WAV audio source in test builds

A build compiled with `--features audio-fixture` (Cargo feature, never enabled
in release) reads `TRANSCREVE_AUDIO_FIXTURE=<path.wav>` at `AudioRecorder::open`
time and feeds that file into the capture ring at real-time pace instead of a
microphone. The full pipeline — resampler, VAD, frame subscribers, recording
buffer — runs unchanged.

- While no dictation session is recording the feeder emits silence.
- Each `Cmd::Start` rewinds the file, so "press shortcut → utterance plays →
  release" is deterministic.
- Works on machines without a microphone (headless runners).

Build a fixture-capable binary (PowerShell, see BUILD.md):

```powershell
. .\scripts\windows-dev-env.ps1 -BypassJunction -TargetDir C:\t-<lane>
cargo build --features audio-fixture
$env:TRANSCREVE_AUDIO_FIXTURE = "C:\multimidia\ecc-t009\e2e\fixtures\pt-br\fala_curta.wav"
```

## Audio fixtures

[`e2e/fixtures/`](fixtures/) holds the pt-BR WAVs shared with T-035 (text
pipeline tests) and later WER/latency benchmarks (T-017). They are generated
deterministically by [`fixtures/generate.py`](fixtures/generate.py) (stdlib
only). The shipped samples are **synthetic speech-like audio** — they exercise
timing, VAD gating and ring transport, but are not intelligible speech; drop-in
real recordings under the same filenames when available.

## Running

```bash
# Webview tests (CI: .github/workflows/playwright.yml)
bun install
bunx playwright install chromium
bun run test:e2e          # alias of `playwright test`

# Rust fixture unit tests
cd src-tauri && cargo test --features audio-fixture fixture

# Native smoke (manual, Windows desktop session — see e2e/native/README.md)
python -m pytest e2e/native -m smoke
```
