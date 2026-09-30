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

## Em andamento (lanes)

- **T-016** `feat/t-016-keyring-secrets` @ `ecc-t016` (70b66b95)
- **fix/review-t005-t020** @ `ecc-fix-review` (95aab676) — watchdog backoff/held/release_all, validate_shortcut vs acordes de colagem, timeout do recording_loop, normalização idempotente de app_language

## Próximas na DAG (prontas para lanes)

- **T-002** [P] CI · **T-003** [P] logging · **T-006** [P] IPC/specta · **T-007** instância única/bandeja · **T-009** [P] E2E + fixtures WAV pt-BR · **T-015** [P] modelos · **T-060** monitor de mic · **T-063** loopback · **T-040** Flow Bar (dep. T-008 ✔) · **T-043** [P] telas de modelos · **T-044** [P] configurações+dicionário · **T-046** privacidade

## Merges pendentes

- (nenhum)

## Bloqueios

- `cargo`/`cmake` nem sempre no PATH do PowerShell — prefixar `$env:Path` se `cargo` falhar após o env script (reportado pela lane T-020).

## Dívida conhecida / fix-forward

- `LLKHF_INJECTED` não é checado pelo crate handy-keys — eventos injetados pelo enigo chegam ao matcher (nota manual; candidate a patch upstream).
- T-031 deve reconciliar `insertion_method` (novo, T-005) com `paste_method` legado que ainda dirige a colagem.
- `src/bindings.ts` precisa ser regenerado num `bun run tauri dev` em algum merge (foi editado à mão na T-005).

## Notas operacionais

- Conflitos esperados entre lanes: `src/bindings.ts` (regenerar após cada merge), `src/i18n/locales/en/translation.json`, `specs/tasks.md`.
- Windows: cargo/`bun run tauri` só em PowerShell com `. .\scripts\windows-dev-env.ps1 -BypassJunction`.
- Catracas da T-001a: clippy `-D warnings`, `cargo deny`/`audit`, ≤12 arquivos >800 linhas, ≤146 unwrap/expect, ≤50 unsafe. Nunca regredir.
