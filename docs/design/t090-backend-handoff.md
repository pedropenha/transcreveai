# T-090 backend handoff — 2026-10-03

## Implemented

- Windows discovery treats npm Codex `.cmd`/`.bat` only as metadata for fixed native package paths (x64/arm64). It never reads or executes the batch content. Other scripts are rejected, native `.exe`/`.com` are launched directly, and unsupported PATH entries do not hide later native installs.
- Override validation and runtime launch both enforce supported native execution. Relative paths, UNC paths and incorrect executable names remain rejected.
- Codex 0.159.3 argv adds `--ignore-user-config --ignore-rules`; auth still uses existing `CODEX_HOME`. Existing `read-only`, ephemeral and stdin-only prompt behavior remains.
- Claude Code 2.1.273 argv adds `--safe-mode` preserving OAuth/session auth while disabling customizations, hooks and plugins. Tools, MCP, slash commands and persistence stay disabled. Version/auth probes also use safe mode. `--bare` remains deliberately unused because it blocks OAuth/keychain auth.
- Extra args now require a conservative exact allowlist: Codex `--color=never`/`--color=auto`; Claude `--effort=low|medium|high|xhigh|max`. Unknown flags, positionals, option delimiters, short clusters and future CLI options fail closed. Validation errors do not echo caller args.
- Each call owns a private empty temporary working directory. Pipe task guards abort detached readers/writers when calls are cancelled, releasing owned prompt buffers. Process-tree cleanup remains active and Windows cleanup helpers run without visible console windows.
- Failed calls log stderr byte count, never stderr content. No prompt is placed in argv, files or logs.

## Evidence

- Installed `codex exec --help` and `claude --help` verified the isolation flags above. Claude `--safe-mode auth status` accepted and verified existing session login without changing it.
- TDD RED: focused run had 33 passing and 12 failing tests, including missing isolation flags, unknown argv acceptance, and existing shell-based fixtures rejected by native-only runtime. The Auto experimental fallback regression separately failed before root's correction.
- GREEN: `cargo test --lib llm::cli_agent::tests -- --test-threads=1` passed 47 tests, with 3 explicitly ignored live tests. Windows process fixtures compile one native Rust executable and copy it under each fixture name; no production/test shell bypass exists.
- Three ignored opt-in tests require `TRANSCREVE_RUN_LIVE_CLI_TESTS=1`: synthetic Summary via Codex subscription, synthetic Summary via Claude subscription, and Codex process cancellation/reaping. They contain no actual user meeting/history. Root owns running these and recording results.
- Independent security review: `docs/design/t090-security-review.md`.
- LLVM line coverage of the CLI production files: `cli_agent.rs` 90.24%, `adapters.rs` 92.31%, `parse.rs` 80.54%, `spawn.rs` 84.03%, `validate.rs` 93.10%. Measured using focused CLI tests (47 pass, 3 live ignored); these are scoped rates, not whole-app coverage.
- Persistent LLVM data: `D:\t\llvm-cov-target\src-tauri.profdata`, sibling `.profraw` files and instrumented binary `D:\t\llvm-cov-target\debug\deps\transcreve_ai_app_lib-f09e16dad41babfc.exe`. The original human-readable report was printed to terminal, not saved as JSON/text. Measurement command after `. .\scripts\windows-dev-env.ps1 -BypassJunction`: `cargo llvm-cov --manifest-path src-tauri/Cargo.toml --lib --ignore-filename-regex 'bindings|tests|vendor' -- llm::cli_agent::tests --test-threads=1`. Regenerate/export existing report without rerunning tests: `cargo llvm-cov report --manifest-path src-tauri/Cargo.toml --ignore-filename-regex 'bindings|tests|vendor'` (same environment sets `CARGO_TARGET_DIR=D:\t`).
- Auto provider selection regression GREEN in instrumented binary: 1/1 pass after root's fix. Root separately reported all three live opt-in tests pass in fresh authenticated environment (19.37 seconds).

## Remaining coordination

- Root should run final integrated library tests (includes corrected Auto provider selection), opt-in live acceptance and final coverage check, and record results. Restore real `LOCALAPPDATA` for live acceptance because the Windows build environment script redirects it to a compiler-only shadow.
- A successful process cancellation test proves spawned process reaping; it does not prove that the remote model had started producing a response.
- Root owns final integrated library/clippy gates; backend focused coverage satisfies the required 80% minimum for each scoped production file.
- No commits or branch switches were made. Existing unrelated changes were preserved.

## Final Rust review (read-only code inspection)

- Reviewed the T-090 CLI delta, root's Auto fallback hunk and regression, and FR-012-01..05/NFR-012-02 compatibility. No remaining actionable correctness blocker found in this scope. Unrelated assistant dictation changes were excluded from this review.
- Largest changed production CLI file is `spawn.rs` at 674 lines, below the 800-line maximum. No new production `unwrap`/`expect` or unsafe block was introduced; its two regex `expect` calls predate this task and operate on constant patterns.
- Drop ordering keeps the private working directory alive through process teardown. Pipe guards abort detached tasks; child guards preserve cancellation cleanup. Errors from filesystem/spawn/wait are surfaced generically; caller-controlled extra args are not echoed.
- Auto now excludes experimental summary providers in the final fallback, while explicit assistant selection still works. The regression tests both experimental adapters and both Auto/explicit behavior; this matches FR-012-04.
- Native npm lookup intentionally covers known Windows x64/arm64 package layouts and fails closed for unknown scripts. Older CLI versions lacking the pinned isolation flags fail rather than weakening protections. Real acceptance covers the installed Windows versions only; macOS/Linux runtime remains outside this evidence.
- Non-blocking clarity debt: a few older validation/tree-cleanup comments still mention batch-shell escaping. They do not affect current native-only behavior. Live cancellation verifies process reaping, not provider response streaming or the full Tauri UI cancel flow.
- No additional builds/tests were run during this final review; root owns the final integrated test/Clippy results.

## Integrated test fixture repair

- Root's full library run found 955 pass, 2 fail, 3 ignored. Both failures used assumptions intentionally removed by T-090 hardening: a Windows batch fixture and an arbitrary extra flag.
- Updated only tests in `commands/llm.rs`: binary status privacy now copies the native test executable under `codex.exe`; normalization uses allowed `--color=never`. The path redaction, blank removal and timeout clamp assertions remain intact. No production command behavior changed. Root owns rerunning the integrated suite after this repair.
