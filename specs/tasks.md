# Plano de implementação (tarefas)

Legenda: `[P]` = paralelizável com as outras `[P]` da fase · **Refs** = requisitos atendidos · marque `[x]` ao concluir.

Base do código: **fork divergente do Handy** ([ADR-0001](../docs/adr/0001-fork-do-handy-como-base.md)). Cada tarefa leva uma destas marcações:

- **Adaptar**: o Handy já tem a funcionalidade. A tarefa é verificar contra os `AC-*`, escrever os testes que faltam, trazer o trecho alterado para o padrão das rules e renomear.
- **Estender**: o Handy tem uma parte. Reaproveitar o que existe e construir o que falta (a lacuna vem descrita).
- **Migrar**: o Handy faz de um jeito que as rules/specs não aceitam; trocar preservando os dados do usuário.
- **Sem marcação**: construir do zero.

Código herdado segue a política do [plan.md §11](architecture/plan.md#11-engenharia-governada-pelas-ecc-rules): as rules valem para o que for alterado, e a cobertura sobe em catraca.

## Escopo v1

Conforme o [ADR-0002](../docs/adr/0002-escopo-v1.md), o release v1 = **ditado + notetaker de reuniões + todas as telas**. Tarefas marcadas **(v1.1+)** estão fora do escopo; as demais listadas nas fases 0–3 são v1. O release é um `bun run tauri build` local (NSIS, sem assinatura/updater — T-049 não se aplica).

## Fluxo de cada tarefa

Não há definição de pronto própria: vale o workflow das ECC rules (`rules/common/development-workflow.md`, `testing.md`, `code-review.md`, `git-workflow.md`). Na prática:

1. **Pesquisa/reuso** antes de adicionar dependência ou código novo — skill `search-first`.
2. **Planejamento** para tarefas complexas — `ecc:planner`.
3. **TDD** (RED → GREEN → REFACTOR) com a cobertura exigida pelas rules — `tdd-workflow` / `ecc:tdd-guide`, `rust-testing`, `react-testing`. Os `AC-*` da spec viram os testes.
4. **Revisão** — `ecc:code-reviewer` + `ecc:rust-reviewer` / `ecc:typescript-reviewer` / `ecc:react-reviewer`; `ecc:security-reviewer` nos gatilhos de segurança.
5. **Verificação** — `verification-loop` com resultado PASS.
6. **Commit** convencional (`feat:`, `fix:`…).
7. Se o **comportamento de produto** mudou, atualizar a spec da feature.

---

## Fase 0 — Fundação

- [x] **T-000** **Avaliar fork do Handy × começar do zero.** Resultado: **fork divergente** do Handy (commit `29bd2c0`) com cherry-pick seletivo das correções do upstream. A avaliação dos 6 critérios está no [ADR-0001](../docs/adr/0001-fork-do-handy-como-base.md). Foi feita só lendo o código (sem toolchain Rust na máquina), por isso a T-001 tem um portão de execução.
- [x] **T-001** **Fork + rebranding + portão no Windows.**
  - Criar o repositório a partir do Handy em `29bd2c0`, com remote `upstream`.
  - Renomear para **Transcreve.ai** (`identifier` `br.com.creator4all.transcreve.ai`; slug `transcreve-ai`; variáveis de ambiente `TRANSCREVE_*`): nome, ícones e logo (barras de som), textos, `sponsor-images/`. Updater: remover o endpoint e a chave do Handy e deixar o updater desligado até a T-049. Manter o `LICENSE` MIT com o copyright do Handy e acrescentar `Copyright (c) 2026 Creator4all`. O empacotamento Linux/macOS herdado (incluindo Nix) é mantido e renomeado, não removido.
  - Janela principal do Handy = `hub`; overlay `recording_overlay` = `flowbar`.
  - Instruções para agentes: o `CLAUDE.md` herdado aponta para o `AGENTS.md` do Handy. Reescrever o `CLAUDE.md` para apontar para `specs/` e para a hierarquia da [constituição](constitution.md) (ECC rules > specs). Do `AGENTS.md`, manter os comandos e a visão da arquitetura; remover o fluxo de PR/issues do repositório do Handy. Avaliar remover o `CRUSH.md`.
  - Bloquear push acidental para o Handy: `git remote set-url --push upstream no_push`.
  - Toolchain Windows (conforme o `BUILD.md` do Handy), documentada no README: Rust stable (rustup), Bun, Visual Studio Build Tools 2022 com a carga "Desenvolvimento para desktop com C++" (MSVC), CMake no `PATH` e Vulkan SDK (LunarG; define `VULKAN_SDK`, usado pelo backend Vulkan do whisper). Após instalar, abrir um terminal novo.
  - **Portão (smoke manual no Windows 11)**: `bun run tauri dev` sobe; ditado local chega ao Bloco de Notas, ao Chrome e ao VS Code; o clipboard anterior é restaurado; o overlay não rouba o foco; `cargo test` passa. Se falhar de forma estrutural, reabrir o ADR-0001.
  - Depende de: T-000.
- [x] **T-001a** **Baseline ECC do código herdado.**
  - `cargo llvm-cov` e cobertura do frontend para registrar a baseline.
  - `clippy -D warnings`: corrigir, ou usar `allow` com justificativa.
  - Configurar `cargo deny` (fontes git permitidas explicitamente e pinadas por `rev`) e `cargo audit`.
  - Inventariar arquivos com mais de 800 linhas e `unwrap` em produção.
  - Revisar os 50 usos de `unsafe` com `ecc:security-reviewer`.
  - Fins de linha: com `core.autocrlf=true`, o checkout no Windows vem em CRLF e o `format:check` (Prettier com `endOfLine: lf`) falha em todos os arquivos. Adicionar `.gitattributes` (`* text=auto eol=lf`).
  - Saída: seção "Exceções herdadas" no ADR-0001 e a meta de catraca.
  - Depende de: T-001. — Refs: `rules/common/testing.md`, `rules/common/code-review.md`, `rules/rust/*`
- [ ] **T-002** [P] **Estender** CI do Handy. Hoje: `cargo test` só no Ubuntu, ESLint, Prettier, checagem de traduções, smoke de Playwright e builds por plataforma. Acrescentar: job `windows-latest`, `clippy -D warnings`, cobertura (`cargo llvm-cov` + testes do frontend) com catraca sobre a baseline da T-001a, `cargo audit`, `cargo deny check`. Depende de: T-001a. — Refs: FR-011-26
- [ ] **T-003** [P] **Adaptar** logging (`log` + `tauri-plugin-log`): auditar os logs herdados para garantir que não gravam texto transcrito, prompts nem chaves; configurar o caminho `%LOCALAPPDATA%\br.com.creator4all.transcreve.ai\logs` (`app_log_dir` do Tauri). — Refs: FR-011-03, AC-011-03
- [x] **T-004** [P] **Adaptar** SQLite (`rusqlite` + `rusqlite_migration`, `managers/history.rs`): alinhar o schema ao [data-model](architecture/data-model.md) com migrações novas, sem perder o histórico; domínios novos com repositórios por trait. — Refs: [data-model](architecture/data-model.md)
- [x] **T-005** [P] **Adaptar** settings (`settings.rs`, `settings.json` via `tauri-plugin-store`) para o schema versionado do data-model; i18n: o Handy tem `pt` e mais 25 idiomas. Manter **só pt-BR e en** (decisão do ADR-0002); remover os demais locales (skill `i18n-sync`). — Refs: NFR-010-03
- [ ] **T-006** [P] **Adaptar** IPC: `tauri-specta` já gera `src/bindings.ts`; alinhar comandos e erros ao envelope `ok/error` do contrato. — Refs: [contracts §5](architecture/contracts.md#5-ipc-tauri)
- [ ] **T-007** **Adaptar** instância única, autostart e bandeja (já existem no Handy): verificar contra os AC e ajustar o menu à FR-010-14. — Refs: FR-010-13..15, FR-010-18
- [x] **T-008** [P] **Direção de design** (`rules/web/design-quality.md`): estilo, paleta, tipografia e tokens a partir dos prints do Wispr, com as skills `frontend-design-direction` / `design-system`. Saída: `DESIGN.md` + tokens. Antes de T-040/T-042. — Refs: F001, NFR-010-05
- [ ] **T-009** [P] **Base de testes E2E**: Playwright nas webviews (`e2e-testing`) + `windows-desktop-e2e` (pywinauto/UI Automation) para fluxos nativos, com fonte de áudio WAV em builds de teste. Inclui fixtures WAV pt-BR (fala curta/longu com muletas) usados também pela T-035. — Refs: `rules/common/testing.md`, F002 notas técnicas

## Fase 1 — Ditado (v1)

### Áudio e STT

- [x] **T-010** **Adaptar** engine de áudio (`audio_toolkit/audio/`, `managers/audio.rs`: cpal + rtrb + rubato, visualizer → evento `mic-level`): verificar reamostragem 16 kHz, níveis a 30 Hz, troca de dispositivo e fan-out. — Refs: FR-002-10, FR-002-20, FR-001-05
- [ ] **T-011** **Adaptar** VAD (Silero v4 + Earshot, `audio_toolkit/vad/`): aparar silêncio e "nada ouvido". — Refs: FR-002-14, FR-003-11
- [x] **T-012** **Estender** (refatorar) `managers/transcription.rs` (2.529 linhas, centrado no motor local) para o trait `SttProvider` + orquestrador com retry/fallback, dividindo o arquivo por domínio. — Refs: [contracts §2](architecture/contracts.md#2-sttprovider), FR-003-16
- [ ] **T-013** [P] **Adaptar** provedor local whisper (`transcribe-cpp`): ciclo de vida do modelo e descarregar por ociosidade (existem), dicas de vocabulário (`custom_words`); verificar ou construir o filtro de alucinação. — Refs: FR-003-09, FR-003-12, FR-003-13, AC-003-05
- [ ] **T-014** [P] **(v1.1+)** Provedores `openai`, `groq`, `openai_compat` (multipart, divisão > 20 MB, timeouts). O Handy não tem STT em nuvem. Adiada: na v1 só existe STT local (ADR-0002). — Refs: FR-003-14..16, AC-003-07
- [ ] **T-015** [P] **Adaptar** gerenciador de modelos (`managers/model.rs`, `catalog/`: download retomável + SHA-256 já existem): verificar detecção de hardware, importar/excluir e espaço em disco. — Refs: FR-003-05..08, AC-003-01, AC-003-08, FR-011-24
- [ ] **T-016** [P] **Migrar** segredos: hoje o Handy guarda `post_process_api_keys` em texto puro no `settings.json`. Mover para o cofre do SO (`keyring`), com `secret_set`/`secret_clear`, e remover as chaves do JSON na migração. Na v1 cobre a chave do LLM BYOK (T-050, resumo de reunião). Passa por `ecc:security-reviewer`. Pré-requisito da v1. — Refs: FR-011-01..05, AC-011-01
- [ ] **T-017** **(v1.1+)** Baseline de WER/latência com a skill `benchmark` sobre os fixtures WAV da T-009. — Refs: NFR-003-02

### Atalhos e sessão

- [x] **T-020** **Adaptar** hook de teclado (`handy-keys` numa thread dedicada, fallback Tauri): verificar watchdog e callback < 1 ms. — Refs: NFR-002-01, NFR-002-03
- [ ] **T-021** **Estender** matcher (`shortcut/`): combos só de modificadores e cancelamento dinâmico já existem. Faltam promoção de modo por prefixo, duplo toque, supressão e menu mask key. — Refs: FR-002-01..08, AC-002-01..08
- [ ] **T-022** **Estender** `transcription_coordinator.rs` (`Idle/Recording/Processing` + toque pendente) para a máquina completa (`Arming`, `Transcribing`, `Inserting`…), fila FIFO de N sessões (N = 5 pendentes, padrão), limite de duração (padrão 5 min, configurável 1–20 no Avançado), preservação de áudio e comando "enviar" (partir de `auto_submit`). — Refs: FR-002-09..19, AC-002-09..11

### Inserção

- [ ] **T-030** **Adaptar** inserção `paste` (`paste_tx/windows.rs` já faz snapshot multi-formato, exclusão do histórico/nuvem do clipboard e restauração condicionada a `GetClipboardSequenceNumber`): verificar a espera de liberação de modificadores (FR-005-01) e os AC. — Refs: FR-005-01..04, AC-005-01..04, AC-005-08
- [ ] **T-031** [P] **Estender** métodos: Ctrl+V, Shift+Insert, Ctrl+Shift+V, digitação direta (`enigo`) e nenhum já existem. Faltam `auto` (que passa a ser o **padrão**, ADR-0002), detecção de janela elevada (UIPI) → `clipboard_only` com aviso, `newline_mode` e registro do resultado. — Refs: FR-005-05..11, AC-005-05, AC-005-07
- [ ] **T-032** **(v1.1+)** Matriz de apps automatizada (AC-005-06) com `windows-desktop-e2e`. Na v1, a cobertura é o checklist de smoke manual entregue no fim do loop. Depende de: T-009.

### Pipeline mínimo

- [ ] **T-035** **Estender** pipeline com as etapas 1, 2 e 4 (vocab) + limpeza determinística `light`, partindo de `custom_words` e da remoção de vícios em `audio_toolkit/text.rs`, num módulo `pipeline/` puro. A `light` remove muletas pt-BR ("né", "tipo", "aí", "ahn/ééé", "então assim", repetições imediatas) e corrige pontuação/capitalização; a lista é editável na tela Dicionário (T-044). — Refs: FR-004-01..03, FR-004-08, FR-004-12, AC-004-01, AC-004-10

### UI

- [ ] **T-040** **Estender** `overlay.rs` + `src/overlay/` para a Flow Bar. Já existem: não-focável, topmost reaplicado, transparente, níveis e botão cancelar. Faltam: click-through na área transparente, hover com 2 botões (Ditar + Notetaker, F009 — o botão de reunião já existe na v1) e tooltip de atalho, estados da F001, posição inferior-centro (padrão). Depende de: T-008. — Refs: FR-001-01..06, NFR-001-01..05, AC-001-01..03, AC-001-07..08
- [ ] **T-041** [P] Flow Bar: menu de clique direito, arrastar/encaixar, multi-monitor, visibilidade, tela cheia, soneca (15/30/60 min), sons (ligados por padrão, desativáveis). — Refs: FR-001-07..13, AC-001-04..06
- [ ] **T-042** [P] **Estender** Hub + Início/Histórico: a lista de histórico existe. Faltam FTS5, filtros, detalhe com diff, estatísticas e o layout da T-008. — Refs: FR-010-01..06, AC-010-03
- [ ] **T-043** [P] Modelos & Provedores — **só modelos locais na v1** (listar, baixar, importar, excluir, selecionar para ditado/reunião). Adicionar/testar provedores em nuvem volta na v1.1+ com a T-014. — Refs: FR-003-01..04, AC-003-01, AC-003-08
- [ ] **T-044** [P] **Estender** Configurações (Geral com captura de atalho e medidor de mic; Sistema; Privacidade; Avançado — inclui limite de gravação e tamanho da fila de sessões) + Dicionário (vocab + lista editável de muletas da limpeza `light`, T-035), reorganizando as telas herdadas conforme a F010. — Refs: FR-010-07..10, FR-010-12, FR-002-02..03, FR-004-12
- [ ] **T-045** **Adaptar** onboarding herdado: incluir o campo de prática e a opção de reabrir. Na etapa de transcrição, o usuário **escolhe o modelo local**; `large-v3-turbo` é o default recomendado (a detecção de hardware informa o rótulo, não impõe). Sem opção de nuvem na v1. — Refs: FR-010-16..17, FR-003-05, AC-010-01..02
- [ ] **T-046** Modo offline, "O que é enviado", retenção, "Apagar todos os dados" (inclui o áudio preservado de sessões falhas, T-022). — Refs: FR-011-06..13, AC-011-02, AC-011-04, AC-011-06

### Release

- [ ] **T-049** **(v1.1+)** **Adaptar** pipeline de release do Handy (`build.yml`/`release.yml`: NSIS, assinatura das DLLs, updater) com certificado, chave do updater e endpoint próprios. Na v1, o release é `bun run tauri build` local sem assinatura/updater (ADR-0002). Herdado da T-001: preencher `plugins.updater` (`pubkey`/`endpoints`) e voltar `createUpdaterArtifacts` para `true` no `tauri.conf.json`, o que reativa o updater via `updater_policy.rs` (remover o teste `shipped_config_has_no_channel_until_t049`); apontar `PORTABLE_RELEASES_URL` (`portableInstaller.ts`) para as nossas releases; trocar a versão 0.9.7 herdada; revisar nos workflows a assinatura Azure do Handy e os downloads de `blob.handy.computer`. — Refs: FR-011-25, FR-010-19

## Fase 2 — Texto inteligente (v1.1+)

> Fora do escopo v1 (ADR-0002), exceto a T-050, promovida para a Fase 3 por ser pré-requisito do resumo de reunião.

- [ ] **T-051** **(v1.1+)** **Estender** o pós-processamento herdado para a limpeza por LLM: prompts versionados, níveis, backtrack, salvaguardas, timeout/fallback. — Refs: FR-004-11..16, AC-004-01..04, AC-004-06, AC-004-09
- [ ] **T-052** [P] **(v1.1+)** Snippets + tela. — Refs: FR-004-04..07, AC-004-05
- [ ] **T-053** [P] **(v1.1+)** Dicionário com substituições + tela. — Refs: FR-004-09
- [ ] **T-054** [P] **(v1.1+)** Perfis de app/estilos + tela. — Refs: FR-004-17..20, AC-004-07..08
- [ ] **T-055** **(v1.1+)** Evals de prompts (limpeza e comando) com a skill `eval-harness`. — Refs: NFR-004-02, FR-006-13
- [ ] **T-056** **(v1.1+)** Command Mode: captura de seleção, prompt, substituir/gerar. — Refs: FR-006-01..11, AC-006-01..05
- [ ] **T-057** [P] **(v1.1+)** Notas por voz + Scratchpad (editor, busca, fixar, copiar Markdown). — Refs: FR-007-01..08, AC-007-01..04

## Fase 3 — Reuniões (v1)

- [ ] **T-050** **Estender** `llm_client.rs` (OpenAI-compatível + Anthropic já existem) para o trait `LlmProvider`; teste de conexão; roteamento conforme `cost-aware-llm-pipeline`. **Na v1 serve só ao resumo de reunião (BYOK)**: sem chave configurada, transcrição e notas funcionam e o resumo fica desabilitado (FR-009-21). Depende de: T-016. — Refs: [contracts §3–4](architecture/contracts.md#3-llmprovider), FR-009-17..21

- [ ] **T-060** Monitor de uso do microfone (registro ConsentStore) + snapshots de janelas. — Refs: FR-008-01
- [ ] **T-061** Detector (regras, debounce, memória de título, fim de reunião). Regras embutidas na v1: **Zoom, Teams, Meet (navegador), Webex**; demais apps da F008 ficam como regras v1.1+. — Refs: FR-008-02..06, FR-008-13..14
- [ ] **T-062** Toast não-ativável (compacto/expandido, ▾, recolher). — Refs: FR-008-07..12, AC-008-01..03, AC-008-07
- [ ] **T-063** Captura loopback + troca de dispositivo + gravação em blocos + recuperação. — Refs: FR-009-03..05, AC-009-04..05
- [ ] **T-064** Sessão de reunião: iniciar/pausar/parar, limites, check-in, indicador, consentimento. — Refs: FR-009-01..02, FR-009-06..09, AC-009-08
- [ ] **T-065** Transcrição ao vivo por trilha + coexistência com ditado. — Refs: FR-009-10, FR-009-15, AC-009-03
- [ ] **T-066** Janela da reunião (Minhas notas, Transcrição, Resumo). — Refs: FR-009-12..14, AC-009-02
- [ ] **T-067** Pós-processamento: resumo, map-reduce, título, regenerar. — Refs: FR-009-16..22, AC-009-01, AC-009-06
- [ ] **T-068** [P] Copiar como Markdown + lista/busca de reuniões. — Refs: FR-009-23, FR-009-25, AC-009-07
- [ ] **T-069** Auto-start/auto-stop + **roteiro manual parcial** dos apps de reunião (Zoom, Teams, Meet, Webex) — o que não dá para automatizar vira checklist de smoke entregue no fim do loop. — Refs: AC-008-04..06, AC-008-08

## Fase 4 — v1.1+ (pós-v1)

- [ ] **T-080** [P] **(v1.1+)** **Adaptar** Parakeet local (já vem do Handy via `transcribe-rs`/ONNX): verificar pt-BR e expor atrás do `SttProvider`. Pode ser antecipada. — Refs: F003
- [ ] **T-081** [P] **(v1.1+)** **Adaptar** whisper com Vulkan (herdado: `transcribe-cpp` com backends dinâmicos no x86_64, dispositivo `Auto` com fallback para CPU): verificar o fallback automático, o log e o rótulo na UI. — Refs: FR-003-10
- [ ] **T-082** [P] Diarização da trilha `system` + deduplicação de eco. — Refs: FR-003-19, FR-009-11
- [ ] **T-083** [P] Provedor Deepgram. — Refs: F003
- [ ] **T-084** Port macOS e Linux (validar build, atalhos, colagem e overlay; o código herdado já compila nas duas plataformas). Herdado da T-001: validar o empacotamento Nix renomeado (`flake.nix`, `nix/`, cache `handy-computer` no `nix-check.yml`) e trocar o link de ajuda do Secure Input no macOS (`SecureInputWarning.tsx`, hoje na documentação do Handy).
- [ ] **T-085** Auditoria de acessibilidade (`accessibility`) e latência (`latency-critical-systems`, `benchmark`).

---

## Dependências (resumo)

```mermaid
flowchart LR
  T000 --> T001
  T001 --> T001a --> T002
  T001 --> T010 & T020 & T040 & T042
  T008 --> T040 & T042
  T009 --> T032
  T004 --> T022 & T042
  T010 --> T011 --> T022
  T012 --> T013 & T014
  T016 --> T014 & T050
  T020 --> T021 --> T022
  T022 --> T030 --> T031
  T022 --> T035
  T040 --> T041
  T043 --> T045
  T050 --> T051 & T056 & T067
  T060 --> T061 --> T062
  T063 --> T064 --> T065 --> T067
```
