# LOOP-STATE — Transcreve.ai v1

> Memória do loop entre sessões. Atualizar ao fim de cada task: task atual, lanes, merges pendentes, bloqueios.
> Governança: [ADR-0002](docs/adr/0002-escopo-v1.md) · plano: [specs/tasks.md](specs/tasks.md)

## Estado

- **Alvo de merges**: `integration/v1` (worktree `C:\multimidia\ecc-v1`). `main` nunca é tocada.
- **Base**: `chore/t-001a-baseline` + `chore/t-008-design-direction` (já mergeada em `3a82c14`).
- **Última atualização**: 2026-09-30 — ondas 1+2 mergeadas (11 tasks + 2 fix-forwards).

## Concluído

- [x] Passo 0 — Governança: `integration/v1` criada em `ecc-v1`; ADR-0002; `specs/tasks.md` emendado; specs corrigidas; `bindings.ts` commitado (`2179f07`).
- [x] T-001, T-001a, T-008 (pré-loop)
- [x] **T-005** — merge `a79ec5f`. Schema v3 + só en/pt-BR (24 locales removidos, `pt`→`pt-BR`). bindings.ts editado à mão — regenerar num dev-build.
- [x] **T-020** — merge `c9a53f2`. `shortcut/matcher.rs` novo (matcher puro); watchdog com backoff; fallback Tauri real. Security review achou 1 HIGH (watchdog flapping no Windows) + 2 MEDIUM → fix-forward na lane `fix/review-t005-t020` @ `ecc-fix-review`.
- [x] **T-004** — merge (ver git log). Schema v9: `dictations` + 11 tabelas novas + 3 FTS5 + triggers; repos por domínio em `src-tauri/src/db/`; `history.rs` reescrito sobre `dictations` sem mudar `HistoryEntry`/IPC. 315 testes.
- [x] **T-010** — merge (ver git log). Evento `audio://level {rms}` (FR-001-05); fan-out `Vec<FrameSubscriber>` com `FrameTap::{Raw,Processed}`+`when_idle` p/ T-063; `ResamplerInitError`; `plan_microphone_resolution` puro. 299 testes.
- [x] **T-012** — merge (ver git log). `managers/transcription.rs` → diretório (engine/inference/language/postprocess/streaming) + `stt/` (trait `SttProvider`, `SttOrchestrator` c/ retry+fallback, `LocalSttProvider`). FR-003-16 implementada. 294 testes.
- [x] **fix/review-t005-t020** — merge (ver git log). `RestartBackoff` puro (backoff também na morte, reset após 30s estável, teto 10 falhas → `shortcut://hook-dead`); `release_all`/`release_binding` no matcher; `InjectionGuard` RAII + `validate_shortcut` rejeitando acordes de colagem; timeout 30s + `emit_to(hub)` no recording_loop; `normalize_app_language` idempotente; carimbo único de schema_version. 309 testes.

- [x] **T-016** — merge `e59fa3b`. Chaves de API no OS keyring (`secrets.rs`, comandos write-only `secret_set/clear/hint`, migração remove plaintext só após cofre confirmar, redator de log no choke point). Reviews acharam 1 HIGH (migração sobrescreve cofre) + MEDIUMs → fix-forward na lane `fix/review-t016` @ `ecc-fix-t016` (cc32c248). `bindings.ts` editado à mão — regenerar.
- [x] **T-002** — merge (ver git log). CI: jobs windows-latest, clippy multi-OS, coverage llvm-cov+frontend c/ catraca ADR-0001, cargo deny/audit, debt-ratchet (`scripts/check-*.ts`). Teto unwrap/expect re-medido: 158.
- [x] **T-003** — merge (ver git log). `redact_secret_patterns` nos erros de LLM, `LLMPrompt` Debug redigido, `redact_text` segue `debug_mode` em runtime (não perfil), log dir/KeepOne confirmados. Ajuste pós-merge: teste de dump não usa mais `post_process_api_keys` (campo removido na T-016).

- [x] **T-006** — merge (ver git log). Envelope ok/error uniforme: ~105 comandos `Result<T,String>` → `Result<T,CommandError {code,message}>`; `CommandErrorCode` tipado; 13 comandos puros envelopados; callers ajustados (`.error.message`). 388 testes.
- [x] **fix/review-t016** — merge (ver git log). Cofre vence migração (sem sobrescrever chave nova); mutex serializando blob de settings; `store:default` removido; memory-store não migra; backoff de retentativa do cofre; comandos `secret_*` async; `ApiKeyField` não grava fragmento da máscara; validação 2560B/control chars; hint ≤len/4; contrato §5 atualizado; teste trava drift de bindings vs specta. 391 testes.
- [x] **T-015** — merge (ver git log). `managers/hardware.rs` (RAM/AVX2/VRAM → tiers + recomendações FR-003-05); `model/disk.rs` (gate de espaço); `import_model` (GGUF probe + sha256 + cópia atômica); turbo = recommended no catálogo; 3 comandos novos reconciliados a CommandError no merge. 398 testes.
- [x] **T-009** — merge (ver git log). Playwright nas webviews (`tests/helpers/tauri-mock.ts` + 3 specs); harness nativo pywinauto em `e2e/native/` (smoke manual); feature cargo `audio-fixture` com `TRANSCREVE_AUDIO_FIXTURE` alimentando o ring do recorder; fixtures WAV pt-BR sintéticos. 380 testes (c/ feature).
- [x] **T-013** — merge (ver git log). `prompt.rs` (initial_prompt c/ dedup + budget) + `hallucination.rs` (blocklist pt/en, filtros R0–R3 por energia RMS do segmento). Custom words já chegavam via vocabulary_hints. 437 testes. `bindings.ts` REGENERADO de verdade (teste de drift passa) — dívida da edição manual quitada.
- [x] **T-060** — merge. `meeting/`: ConsentStore (RegNotify + poll 2s) + EnumWindows + classificador puro (browser exige título — FR-008-02) + gates (pause/offline) + seeds Zoom/Teams/Meet/Webex. ~20 testes.
- [x] **T-063** — merge. Loopback WASAPI via cpal (render device → input stream); `meeting/blocks.rs` (WAV 60s fsync por trilha), `capture.rs` (mic FrameTap::Raw+when_idle + system), `recovery.rs` (órfãs→recovered no startup); reattach backoff p/ troca de device.
- [x] **T-022** — merge. Máquina completa Idle→Arming→Recording→Transcribing→Processing→Inserting→Done/Error em `transcription_coordinator/` (transições puras + effect executor); fila FIFO=5, limite 5min c/ aviso T-60s, <300ms → Empty, strip de comando "enviar", PasteLastAction, failed→dictations. 49 testes da máquina. Bug do manifest do exe de teste RESOLVIDO no build.rs (MANIFEST:EMBED).
- [x] **T-007** — merge (ver git log). Menu da bandeja = FR-010-14 (Hub, ditado, reunião desabilitada até T-064, Flow Bar show/hide, pausa de detecção 1h, modo offline, sair); autostart default on; `offline_mode` + `meeting_detection_paused_until_ms` no schema; quit c/ confirmação se gravando; `--no-tray` fecha de verdade; relaunch hidden. 373 testes + `cargo check` pós-merge verde.

## Em andamento (lanes)

- **T-021** `feat/t-021-matcher` @ `ecc-t021` (54045a53)

## Próximas na DAG (prontas para lanes)

- **T-040** Flow Bar (dep. T-008 ✔) · **T-043** [P] telas de modelos · **T-044** [P] configurações+dicionário · **T-046** privacidade · **T-050** LLM BYOK (dep. T-016 ✔) · **T-030/T-031** inserção · **T-035** pipeline (dep. T-009 ✔) · **T-045** onboarding · **T-041/T-042** Flow Bar pt2/Hub · **T-061..T-069** reuniões (dep. T-060/T-063 em andamento)

## Merges pendentes

- (nenhum)

## Bloqueios

- `cargo`/`cmake` nem sempre no PATH do PowerShell — prefixar `$env:Path` se `cargo` falhar após o env script (reportado pela lane T-020).

## Dívida conhecida / fix-forward

- `LLKHF_INJECTED` não é checado pelo crate handy-keys — eventos injetados pelo enigo chegam ao matcher (nota manual; candidate a patch upstream).
- T-031 deve reconciliar `insertion_method` (novo, T-005) com `paste_method` legado que ainda dirige a colagem.
- `src/bindings.ts` regenerado via teste de drift (workaround mt.exe no exe de teste) — quitado. Problema de manifest do exe de teste no Windows (0xC0000139) segue aberto — fix em build.rs pendente.
- Evento `shortcut://hook-dead` emitido mas ninguém escuta — UI de aviso fica para follow-up (T-040/T-041).
- Aviso de fallback de mic (T-010) depende de `toast://show` — pendente de lane de UI.
- `managers/transcription.rs` virou diretório — lanes futuras devem editar os submódulos.
- Infra: `CARGO_TARGET_DIR` compartilhado (`C:\t`) — lanes devem usar `-TargetDir C:\t-<lane>`; fixar `TEMP/TMP` único por lane (race em `temp_dir()`).
- Infra: o disco C: encheu durante a T-009 (0 bytes livres → `ENOSPC` no `bun install` e no `rustc`). `C:\t-t010` (7,3 GB, lane mergeada) foi removido para destravar; `C:\t` ainda ocupa ~26 GB de cache compartilhado — vale um `cargo clean`/purge agendado.

## Notas operacionais

- Conflitos esperados entre lanes: `src/bindings.ts` (regenerar após cada merge), `src/i18n/locales/en/translation.json`, `specs/tasks.md`.
- Windows: cargo/`bun run tauri` só em PowerShell com `. .\scripts\windows-dev-env.ps1 -BypassJunction`.
- Catracas da T-001a: clippy `-D warnings`, `cargo deny`/`audit`, ≤12 arquivos >800 linhas, ≤146 unwrap/expect, ≤50 unsafe. Nunca regredir.
