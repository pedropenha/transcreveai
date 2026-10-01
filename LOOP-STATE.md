# LOOP-STATE — Transcreve.ai v1

> Memória do loop entre sessões. Atualizar ao fim de cada task: task atual, lanes, merges pendentes, bloqueios.
> Governança: [ADR-0002](docs/adr/0002-escopo-v1.md) · plano: [specs/tasks.md](specs/tasks.md)

## Estado

- **Alvo de merges**: `integration/v1` (worktree `C:\multimidia\ecc-v1`). `main` nunca é tocada.
- **Base**: `chore/t-001a-baseline` + `chore/t-008-design-direction` (já mergeada em `3a82c14`).
- **Última atualização**: 2026-09-30 — waves 1–4 mergeadas; ratchet de dívida reparado pós-wave-4.

## Concluído

- [x] Passo 0 — Governança: `integration/v1` criada em `ecc-v1`; ADR-0002; `specs/tasks.md` emendado; specs corrigidas; `bindings.ts` commitado (`2179f07`).
- [x] T-001, T-001a, T-008 (pré-loop)
- [x] **T-002** — CI: jobs windows-latest, clippy multi-OS, coverage llvm-cov+frontend c/ catraca ADR-0001, cargo deny/audit, debt-ratchet (`scripts/check-*.ts`). Teto unwrap/expect re-medido: 158.
- [x] **T-003** — `redact_secret_patterns` nos erros de LLM, `LLMPrompt` Debug redigido, `redact_text` segue `debug_mode` em runtime, log dir/KeepOne confirmados.
- [x] **T-004** — Schema v9: `dictations` + 11 tabelas novas + 3 FTS5 + triggers; repos por domínio em `src-tauri/src/db/`; `history.rs` reescrito sobre `dictations` sem mudar `HistoryEntry`/IPC. 315 testes.
- [x] **T-005** — merge `a79ec5f`. Schema v3 + só en/pt-BR (24 locales removidos, `pt`→`pt-BR`). bindings.ts editado à mão — regenerar num dev-build.
- [x] **T-006** — merge `528f0e6`. Envelope ok/error uniforme com `CommandError` estruturado.
- [x] **T-007** — Menu da bandeja = FR-010-14; autostart default on; `offline_mode` + `meeting_detection_paused_until_ms` no schema; quit c/ confirmação se gravando; `--no-tray` fecha de verdade; relaunch hidden. 373 testes.
- [x] **T-009** — merge `61fd496`. Base E2E + fonte WAV em builds de teste + harness nativo.
- [x] **T-010** — Evento `audio://level {rms}` (FR-001-05); fan-out `Vec<FrameSubscriber>` com `FrameTap::{Raw,Processed}`+`when_idle` p/ T-063; `ResamplerInitError`; `plan_microphone_resolution` puro. 299 testes.
- [x] **T-011** — merge `0459dcc`. "Nada ouvido" via `StopOutcome`/`RecordedClip` no pipeline do coordinator.
- [x] **T-012** — `managers/transcription.rs` → diretório (engine/inference/language/postprocess/streaming) + `stt/` (trait `SttProvider`, `SttOrchestrator` c/ retry+fallback, `LocalSttProvider`). FR-003-16 implementada. 294 testes.
- [x] **T-013** — merge `ae3bb53`. Prompt de vocabulário + filtro de alucinação whisper.
- [x] **T-015** — merge `8b9728d`. Hardware report, import de modelo, gate de disco.
- [x] **T-016** — merge `e59fa3b` + fix-forward `5c65f2e` (cofre vence migração, mutex de escrita, store:default fora, `ApiKeyField`, comandos async). Chaves no OS keyring; write-only; redator de log no choke point.
- [x] **T-020** — merge `c9a53f2` + fix-forward `fix/review-t005-t020`. `shortcut/matcher.rs` novo (matcher puro); watchdog com `RestartBackoff` (reset após 30 s estável, teto 10 falhas → `shortcut://hook-dead`); `InjectionGuard` RAII; `validate_shortcut` rejeita acordes de colagem; `normalize_app_language` idempotente.
- [x] **T-021** — merge `067149d` + reintegração `d4e663b`. Matcher puro: promoção de prefixo (FR-002-05), duplo toque (FR-002-07), interrupção de arming (FR-002-06), menu mask key.
- [x] **T-022** — merge `30e8883`. Máquina de estados completa do coordinator (FR-002-09..19): Arming/Transcribing/Inserting, fila FIFO, limite de duração, preservação de áudio.
- [x] **T-060** — merge `b4b4b11`. Monitor ConsentStore + snapshots de janelas + classificador.
- [x] **T-063** — merge `6d7ba37`. Loopback WASAPI via cpal + blocos WAV + recuperação de reunião.

## Em andamento (lanes)

- (nenhuma)

## Próximas na DAG (prontas para lanes)

- **T-061** detector · **T-062** toast · **T-064** sessão de reunião (dep. T-060/T-063 ✔) · **T-030/T-031** inserção · **T-035** pipeline de texto · **T-040** Flow Bar (dep. T-008 ✔) · **T-041** · **T-042** · **T-043** · **T-044** · **T-045** · **T-046** · **T-050** (dep. T-016 ✔) · **T-065–T-069**

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

## Reparo pós-wave-4 (review)

- Ratchet de dívida havia regredido (15 arquivos >800 linhas, 163 unwrap/expect, 68 unsafe) — reparado: testes extraídos de `matcher.rs`/`secrets.rs`/`text.rs`, `machine/tests.rs` dividido, blocos `unsafe` contíguos consolidados, `lock().unwrap()` → `unwrap_or_else(|e| e.into_inner())` nas travas internas.
- Bug corrigido: evento de notificação do ConsentStore era manual-reset e nunca era resetado → busy-loop após a primeira mudança; agora auto-reset (`meeting/consent.rs`).
- Bug corrigido: `build.rs` emitia `/MANIFEST:EMBED` global → manifest duplicado com o `resource.lib` do tauri-build (LNK1123 no `tauri build`). Agora o manifest sai do resource.lib (`new_without_app_manifest`) e é embutido via linker em bins e testes.
- `bun run tauri build` verde: `C:\t\release\bundle\{nsis,msi}\` produziram `Transcreve.ai_0.9.7_x64-setup.exe` e `.msi`. Smoke manual pendente (checklist no fim do loop).

## Notas operacionais

- Conflitos esperados entre lanes: `src/bindings.ts` (regenerar após cada merge), `src/i18n/locales/en/translation.json`, `specs/tasks.md`.
- Windows: cargo/`bun run tauri` só em PowerShell com `. .\scripts\windows-dev-env.ps1 -BypassJunction`.
- Catracas da T-001a: clippy `-D warnings`, `cargo deny`/`audit`, ≤12 arquivos >800 linhas, ≤158 unwrap/expect (re-medido), ≤50 unsafe. Nunca regredir.
