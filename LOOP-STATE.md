# LOOP-STATE — Transcreve.ai v1

> Memória do loop entre sessões. Atualizar ao fim de cada task: task atual, lanes, merges pendentes, bloqueios.
> Governança: [ADR-0002](docs/adr/0002-escopo-v1.md) · plano: [specs/tasks.md](specs/tasks.md)

## Estado

- **Alvo de merges**: `integration/v1` (worktree `C:\multimidia\ecc-v1`). `main` nunca é tocada.
- **Base**: `chore/t-001a-baseline` + `chore/t-008-design-direction` (já mergeada em `3a82c14`).
- **Última atualização**: 2026-09-30 — Passo 0 concluído.

- **Última atualização**: 2026-09-30 — Passo 0 + merges T-005 e T-020.

## Concluído

- [x] Passo 0 — Governança: `integration/v1` criada em `ecc-v1`; ADR-0002; `specs/tasks.md` emendado; specs corrigidas; `bindings.ts` commitado (`2179f07`).
- [x] T-001, T-001a, T-008 (pré-loop)
- [x] **T-005** — merge `a79ec5f`. Schema v3 + só en/pt-BR (24 locales removidos, `pt`→`pt-BR`). bindings.ts editado à mão — regenerar num dev-build.
- [x] **T-020** — merge `c9a53f2`. `shortcut/matcher.rs` novo (matcher puro); watchdog com backoff; fallback Tauri real. Security review achou 1 HIGH (watchdog flapping no Windows) + 2 MEDIUM → fix-forward na lane `fix/review-t005-t020` @ `ecc-fix-review`.
- [x] **T-004** — merge (ver git log). Schema v9: `dictations` + 11 tabelas novas + 3 FTS5 + triggers; repos por domínio em `src-tauri/src/db/`; `history.rs` reescrito sobre `dictations` sem mudar `HistoryEntry`/IPC. 315 testes.
- [x] **T-010** — merge (ver git log). Evento `audio://level {rms}` (FR-001-05); fan-out `Vec<FrameSubscriber>` com `FrameTap::{Raw,Processed}`+`when_idle` p/ T-063; `ResamplerInitError`; `plan_microphone_resolution` puro. 299 testes.
- [x] **T-012** — merge (ver git log). `managers/transcription.rs` → diretório (engine/inference/language/postprocess/streaming) + `stt/` (trait `SttProvider`, `SttOrchestrator` c/ retry+fallback, `LocalSttProvider`). FR-003-16 implementada. 294 testes.
- [x] **fix/review-t005-t020** — merge (ver git log). `RestartBackoff` puro (backoff também na morte, reset após 30s estável, teto 10 falhas → `shortcut://hook-dead`); `release_all`/`release_binding` no matcher; `InjectionGuard` RAII + `validate_shortcut` rejeitando acordes de colagem; timeout 30s + `emit_to(hub)` no recording_loop; `normalize_app_language` idempotente; carimbo único de schema_version. 309 testes.

## Em andamento (lanes)

- **T-016** `feat/t-016-keyring-secrets` @ `ecc-t016` (70b66b95)
- **T-009** `chore/t-009-e2e-base` @ `ecc-t009` — Playwright + mock IPC (`tests/helpers/tauri-mock.ts`), harness nativo `e2e/native/` (pywinauto, smoke manual), feature `audio-fixture` (`TRANSCREVE_AUDIO_FIXTURE`), fixtures WAV pt-BR sintéticos em `e2e/fixtures/pt-br/`.

## Próximas na DAG (prontas para lanes)

- **T-002** [P] CI · **T-003** [P] logging · **T-006** [P] IPC/specta · **T-007** instância única/bandeja · **T-015** [P] modelos · **T-060** monitor de mic · **T-063** loopback · **T-040** Flow Bar (dep. T-008 ✔) · **T-043** [P] telas de modelos · **T-044** [P] configurações+dicionário · **T-046** privacidade

## Merges pendentes

- (nenhum)

## Bloqueios

- `cargo`/`cmake` nem sempre no PATH do PowerShell — prefixar `$env:Path` se `cargo` falhar após o env script (reportado pela lane T-020).

## Dívida conhecida / fix-forward

- `LLKHF_INJECTED` não é checado pelo crate handy-keys — eventos injetados pelo enigo chegam ao matcher (nota manual; candidate a patch upstream).
- T-031 deve reconciliar `insertion_method` (novo, T-005) com `paste_method` legado que ainda dirige a colagem.
- `src/bindings.ts` precisa ser regenerado num `bun run tauri dev` em algum merge (foi editado à mão na T-005).
- Evento `shortcut://hook-dead` emitido mas ninguém escuta — UI de aviso fica para follow-up (T-040/T-041).
- Aviso de fallback de mic (T-010) depende de `toast://show` — pendente de lane de UI.
- `managers/transcription.rs` virou diretório — lanes futuras devem editar os submódulos.
- Infra: `CARGO_TARGET_DIR` compartilhado (`C:\t`) — lanes devem usar `-TargetDir C:\t-<lane>`; fixar `TEMP/TMP` único por lane (race em `temp_dir()`).
- Infra: o disco C: encheu durante a T-009 (0 bytes livres → `ENOSPC` no `bun install` e no `rustc`). `C:\t-t010` (7,3 GB, lane mergeada) foi removido para destravar; `C:\t` ainda ocupa ~26 GB de cache compartilhado — vale um `cargo clean`/purge agendado.

## Notas operacionais

- Conflitos esperados entre lanes: `src/bindings.ts` (regenerar após cada merge), `src/i18n/locales/en/translation.json`, `specs/tasks.md`.
- Windows: cargo/`bun run tauri` só em PowerShell com `. .\scripts\windows-dev-env.ps1 -BypassJunction`.
- Catracas da T-001a: clippy `-D warnings`, `cargo deny`/`audit`, ≤12 arquivos >800 linhas, ≤146 unwrap/expect, ≤50 unsafe. Nunca regredir.
