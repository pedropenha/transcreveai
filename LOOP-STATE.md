# LOOP-STATE — Transcreve.ai v1

> Memória do loop entre sessões. Atualizar ao fim de cada task: task atual, lanes, merges pendentes, bloqueios.
> Governança: [ADR-0002](docs/adr/0002-escopo-v1.md) · plano: [specs/tasks.md](specs/tasks.md)

## Estado

- **Alvo de merges**: `integration/v1` (worktree `C:\multimidia\ecc-v1`). `main` nunca é tocada.
- **Base**: `chore/t-001a-baseline` + `chore/t-008-design-direction` (já mergeada em `3a82c14`).
- **Última atualização**: 2026-10-02 — **notetaker completo (T-061–T-069 mergeadas)**. Ondas A/B/C mergeadas em série; verificação final verde: 779 testes lib, clippy `-D warnings`, lint/build/translations/test:unit ok, bindings regen, catracas ok (13→12 arquivos >800 linhas após split `db/meeting_app_rules.rs`). Roteiro de smoke manual entregue em `docs/smoke/notetaker.md` — pendente de execução pelo usuário com apps reais.

## Em andamento (lanes)

- (nenhuma)

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
- [x] **T-030** — merge `4136e1e`. Espera de liberação de modificadores (FR-005-01) + reliable paste como padrão.
- [x] **T-031** — merge `d055a83`. `insertion_method` (auto/paste/typing/clipboard_only) + fallback UIPI + `newline_mode` + `InsertionReport`. Conflito com T-030 resolvido: `paste_text_direct` virou `insertion::type_text_direct`; o await de modificadores foi reintegrado em `type_text_direct` e `send_return_key`.
- [x] **T-035** — merge `a1db877`. `pipeline/` puro (normalize → voice commands → vocab → `light`) + comandos de muletas. Conflito com T-031 resolvido: gatilho do "enviar" = `processed.press_enter`, gate = `method_inserts`.
- [x] **T-040** — merge `7439760`. Flow Bar: click-through, hover Ditar/Notetaker, posição inferior-centro.
- [x] **T-043** — merge `bb30a12`. Tela Modelos & Provedores, só locais na v1.
- [x] **T-050** — merge `403ddfb`. Trait `LlmProvider` + roteamento cost-aware (BYOK).
- [x] **T-064** — merge `8aabfe7`. Sessão de reunião: `meeting_start/pause/resume/stop`, consentimento FR-009-02, limite FR-009-08, check-in FR-009-09, indicador Flow Bar/bandeja; consome `notetaker://start-requested` + `detector://start-requested`; emite `meeting://state` + `meeting://process-requested`.
- [x] **T-061** — merge `7493e9b`. Detector: máquina pura (debounce 5 s, memória de título 10 min, grace 15 s), `detector://meeting`, `detector_respond`, regras Zoom/Teams/Meet/Webex em `meeting_app_rules`, `detector://start-requested`.
- [x] **T-062** — merge `92ff749` + fix `a085971` (`toast::init` realocado p/ `initialize_core_logic`, `meeting_toast_sound` deduplicado). Toast não-ativável compacto/expandido, colapso 60 s → ponto âmbar, supressão DND/tela cheia.
- [x] **T-065** — merge `9aced0b`. Transcrição ao vivo por trilha (`meeting_blocks`, fila pendente, STT local), intervalos de ditado excluídos do mic com `dictation_marker` (FR-009-10), gate por setting (FR-009-15).
- [x] **T-067** — merge `61adcad` + consolidação `043aa62` (colisão de migration: schema `meeting_blocks` unificado no da T-065 + `list_by_meeting`). Pós-processamento: transcrição pendente, resumo map-reduce, título sugerido, fallback sem chave (FR-009-16..22).
- [x] **T-066** — merge `253be5e`. Janela da reunião (notas, transcrição, resumo), edição de título/notas, fechar preserva gravação.
- [x] **T-068** — merge `c2d6575`. Copiar como Markdown (FR-009-23) + lista/busca FTS5 de reuniões (FR-009-25) + UI de lista no hub.
- [x] **T-069** — merge `f0388fc`. Auto-start/auto-stop vinculado ao `detection_id` (FR-008-13..14), "continuar gravando" na janela de stop pendente, `meeting_continue_recording`; checklist de smoke `docs/smoke/notetaker.md`.

## Próximas na DAG (prontas para lanes)

- **T-041** Flow Bar menu/soneca/sons · **T-042** Hub Início/Histórico · **T-044** Configurações + Dicionário (dep. T-035 ✔) · **T-045** onboarding · **T-046** privacidade/retenção
- Depois: smoke manual do notetaker (`docs/smoke/notetaker.md`) + `bun run tauri build` de entrega.

## Merges pendentes

- (nenhum)

## Bloqueios

- `cargo`/`cmake` nem sempre no PATH do PowerShell — prefixar `$env:Path` se `cargo` falhar após o env script (reportado pela lane T-020).

## Dívida conhecida / fix-forward

- `LLKHF_INJECTED` não é checado pelo crate handy-keys — eventos injetados pelo enigo chegam ao matcher (nota manual; candidate a patch upstream).
- `insertion_method` (T-005) reconciliado com `paste_method` na T-031: `resolve_plan` usa `paste_method` como o acorde do método `paste` e `external_script` como escape hatch (`insertion.rs`).
- `src/bindings.ts` — conferir o teste `exported_typescript_bindings_match_checked_in_file` após a onda 5 (foi auto-mergeado nas 6 lanes); regenerar via `bun run tauri dev` se o teste falhar.
- Evento `shortcut://hook-dead` emitido mas ninguém escuta — UI de aviso fica para follow-up (T-041).
- Aviso de fallback de mic (T-010) depende de `toast://show` — pendente de lane de UI (T-062).
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
- Catracas da T-001a: clippy `-D warnings`, `cargo deny`/`audit`, ≤12 arquivos >800 linhas, ≤158 unwrap/expect, unsafe com teto documentado no script (63 atual). Nunca regredir.
- Resíduos conhecidos do notetaker (deferred pelas lanes): atalho global Alt+M não wired; UI de settings de reunião (duração máx., consentimento, check-in) pendente; após aceitar consentimento o usuário dispara o start de novo; evento `shortcut://hook-dead` e aviso de fallback de mic seguem sem listener de UI.
