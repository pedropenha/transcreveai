# ADR-0001: Fork do Handy como base do app (divergente, com cherry-picks)

**Date**: 2026-09-30
**Status**: accepted — com portão de validação no Windows na T-001 (ver Riscos)
**Deciders**: Pedro (dono do produto), Claude (avaliação — T-000, skill `search-first`)

## Context

A T-000 ([specs/tasks.md](../../specs/tasks.md)) pede a decisão entre fazer um fork do [Handy](https://github.com/cjpais/Handy) ou começar do zero, aplicando `search-first` e `rules/common/patterns.md` ("Skeleton Projects: clone best match as foundation"). O Handy usa a mesma stack do [plan.md](../../specs/architecture/plan.md). O MVP (F001–F005, F010) exige integração nativa no Windows: hook de teclado, clipboard, janela não ativável e inferência local. É nessa parte que um projeto novo gasta mais tempo. A avaliação foi feita no commit `29bd2c0` (v0.9.7+, 2026-09-28), clonado fora do projeto. **Não foi possível compilar nem executar** porque a máquina não tem toolchain Rust. Por isso a qualidade no Windows foi avaliada só lendo o código, e a execução real fica como portão da T-001.

### Avaliação dos 6 critérios

**(a) Licença**: MIT. É obrigatório manter o aviso `Copyright (c) 2025 CJ Pais` no `LICENSE` e em cópias substanciais. O README proíbe usar a marca: _"forks … must use their own branding"_. Então o fork precisa trocar nome, ícones, logo, bundle identifier, endpoint do updater e `sponsor-images/`. O nome do produto é **Transcreve.ai**.

**(b) Saúde**: projeto muito ativo. Tem 32,5 mil estrelas e 3 mil forks. Nos últimos 90 dias foram 182 commits de 68 autores, e sai uma release por mês (v0.7.0 em jan/26 → v0.9.7 em set/26). Há 185 issues abertas, sendo 62 com label `bug` e 48 mencionando Windows. O CI tem `cargo test` (só Ubuntu), ESLint, Prettier, checagem de traduções, um smoke de Playwright e builds por plataforma. Riscos: **fator ônibus** (CJ Pais fez cerca de 95% dos commits) e **dependências via git** (forks de `rdev`, `vad-rs`, `rodio`, `hf-hub`, `tao`).

**(c) Cobertura do MVP** (leitura de código):

| Feature             | O que o Handy já tem                                                                                                                                                                                                                                                                                   | Lacunas                                                                                                                                                              |
| ------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ | -------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| F001 Flow Bar       | Overlay transparente, `focusable(false)`, topmost reaplicado via `SetWindowPos(HWND_TOPMOST, SWP_NOACTIVATE)`, fora da taskbar, barras de nível (`mic-level`), botão cancelar                                                                                                                          | Hover com 2 ações, arrastar/encaixar, click-through na área transparente, multi-monitor, ocultar em tela cheia, menu de clique direito                               |
| F002 Atalhos/sessão | `handy-keys` (hook de baixo nível, combos só de modificadores), push-to-talk e toggle, cancelamento dinâmico, coordenador `Idle/Recording/Processing` com toque pendente                                                                                                                               | Supressão, promoção de modo por prefixo, duplo toque, menu mask key, `Arming`, fila FIFO de N sessões, comando "enviar" completo (existe `auto_submit`)              |
| F003 Motores        | whisper (transcribe-cpp, Vulkan no x86_64) e Parakeet/Moonshine (ONNX), catálogo, download retomável com SHA-256, VAD Silero e earshot, descarregar após ociosidade                                                                                                                                    | **Nenhum STT em nuvem**, sem trait `SttProvider` (o manager tem 2.529 linhas e é centrado no motor local), filtro de alucinação a verificar                          |
| F004 Pipeline       | Pós-processamento por LLM (OpenAI-compatível + Anthropic), palavras customizadas com correção por similaridade, remoção de vícios (`audio_toolkit/text.rs`)                                                                                                                                            | Snippets, substituições, perfis de app, salvaguardas de saída, níveis                                                                                                |
| F005 Inserção       | `paste_tx/windows.rs` cobre o núcleo da F005: snapshot multi-formato, `ExcludeClipboardContentFromMonitorProcessing`/`CanIncludeInClipboardHistory`/`CanUploadToCloudClipboard`, restauração condicionada a `GetClipboardSequenceNumber`; métodos Ctrl+V, Shift+Insert, digitação direta, só clipboard | Detecção de janela elevada (UIPI), espera de liberação de modificadores (a verificar), resultado da inserção no histórico                                            |
| F010 Hub            | Histórico em SQLite com migrações, configurações, onboarding, bandeja, autostart, instância única, updater, i18n com `pt` (26 idiomas), bindings TS gerados (`tauri-specta`)                                                                                                                           | Busca full-text/filtros, estatísticas, layout do Hub conforme a T-008, **chaves de API em texto puro no settings store** (viola `rules/common/security.md` e FR-011) |

**(d) Atrito com as ECC rules**: 61 arquivos Rust, cerca de 29 mil linhas.

- **11 arquivos acima de 800 linhas**: `managers/model.rs` 3.090, `managers/transcription.rs` 2.529, `settings.rs` 1.734, `transcription_coordinator.rs` 1.617, `shortcut/mod.rs` 1.426, `lib.rs` 1.122, `managers/audio.rs` 1.101, `actions.rs` 1.053, `recorder.rs` 1.046, `clipboard.rs` 1.015, `overlay.rs` 892.
- **Cerca de 133 `unwrap()` fora de testes**, na maioria `Mutex::lock().unwrap()`. `rules/rust/coding-style.md` diz "never `unwrap()` in production code".
- Erros com `anyhow`/`String`, sem `thiserror`.
- 286 testes Rust, mas **sem medição de cobertura**. No frontend há só 3 testes unitários e 2 de Playwright (smoke).
- O CI não roda `clippy -D warnings`, `cargo audit`, `cargo deny` nem testes no Windows.
- Uso de `log` + `tauri-plugin-log`, o que é aceito por `rules/rust/security.md` ("`tracing` or `log`").

**(e) Domínios novos**: o padrão _manager_ + commands/events do Handy aceita módulos novos. A pipeline de texto e o Command Mode reaproveitam `llm_client`, `paste_tx` e `shortcut`. Reuniões (loopback, monitor de microfone, toast) partem do zero. O custo é igual com ou sem fork. STT em nuvem exige refatorar `managers/transcription.rs` para o trait `SttProvider`.

**(f) Upstream**: o Handy se descreve como _"the most forkable speech-to-text app"_. Features entram via Discussions e a filosofia é evitar inchaço, então nossas features (reuniões, Command Mode, Flow Bar) dificilmente seriam aceitas lá. As refatorações exigidas pelas rules (dividir arquivos, traits, remover `unwrap`) tornam inviável manter merges contínuos.

## Decision

Fazemos **fork do Handy** (`29bd2c0`) como base, renomeado para **Transcreve.ai**, em modo **divergente**. Mantemos um remote `upstream` e fazemos **cherry-pick seletivo** das correções nas áreas ainda compartilhadas (áudio, `paste_tx`, catálogo de modelos, bumps de transcribe-cpp/transcribe-rs, correções de Windows), revisando as releases do Handy uma vez por mês. As ECC rules valem integralmente para código **novo ou alterado**. Para o código herdado, uma tarefa de **baseline** (nova T-001a) mede cobertura, liga os gates de CI e registra as exceções. Um arquivo herdado com mais de 800 linhas é dividido quando for alterado, e a cobertura total sobe em catraca até ≥ 80%.

**Exceção às rules aprovada** pelo dono do produto em 2026-09-30: para o código **herdado**, o mínimo de 80% de cobertura (`rules/common/testing.md`) é atingido em catraca e não antes das features. Nenhum PR pode reduzir a cobertura. Código novo ou alterado cumpre as rules integralmente.

## Alternatives Considered

### Alternativa 1: Começar do zero (scaffold Tauri 2 + React)

- **Pros**: conformidade total com as rules desde o primeiro commit; estrutura de módulos exatamente como no plan.md §3; nenhuma dívida herdada.
- **Cons**: reimplementar cerca de 29 mil linhas já integradas, incluindo correções de Windows difíceis de redescobrir (exclusão do histórico do clipboard, snapshot multi-formato, veto de desligamento no `tao`, crash do ONNX Runtime com AVX2 em CPUs pré-Haswell, topmost reaplicado).
- **Why not**: contraria `rules/common/patterns.md` e o passo 0 de `development-workflow.md` ("prefer adopting or porting a proven approach over writing net-new code"). Atrasaria o MVP em várias fases sem ganho de produto.

### Alternativa 2: Compor (scaffold novo + crates do ecossistema Handy + portar módulos)

- **Pros**: arquitetura própria e limpa; reaproveita `handy-keys`, `transcribe-cpp`, `transcribe-rs`, `vad-rs` e trechos portados (`paste_tx/windows.rs`, `download.rs`) com atribuição MIT.
- **Cons**: a integração (coordenador, overlay, bandeja, updater, onboarding, histórico, settings, i18n) teria de ser refeita; as correções espalhadas em `lib.rs` e `overlay.rs` se perdem.
- **Why not**: o custo de reintegração é maior que o de adequar o código herdado aos poucos. Fica como **plano B** se o portão de validação no Windows (T-001) falhar.

### Alternativa 3: Fork acompanhando o upstream (merges contínuos)

- **Pros**: recebe correções automaticamente.
- **Cons**: impede as refatorações exigidas pelas rules; cada merge vira conflito nos arquivos grandes.
- **Why not**: incompatível com o critério (d). Os cherry-picks cobrem o benefício principal.

## Consequences

### Positive

- Boa parte da Fase 0/1 deixa de ser construída e passa a ser **adaptação e verificação**: T-003, T-004, T-005, T-006, T-007, T-010, T-011, T-013, T-015, T-020, T-030, T-042, T-044, T-045, T-049, e ainda T-050/T-080/T-081 de fases seguintes.
- Correções de plataforma validadas por milhares de usuários vêm de graça.
- Stack confirmada, com crates definidos (ver plan.md §1).

### Negative

- Dívida técnica herdada em relação às rules: arquivos grandes, `unwrap`, ausência de cobertura medida, erros sem tipo. Isso pede uma tarefa de baseline e disciplina de "deixar melhor ao tocar".
- A arquitetura segue a do Handy (`managers/`, `audio_toolkit/`, `commands/`, `shortcut/`, `paste_tx/`) e não o layout ideal do plan.md §3. Domínios novos entram no padrão do Handy.
- Chaves de API em texto puro precisam ser migradas para o cofre do SO antes da v0.1 (T-016 passa a ser **migração**).
- Obrigação de rebranding completo e de manter a atribuição MIT.

### Risks

- **Qualidade no Windows não validada em execução**. Mitigação: portão na T-001 (build local + smoke: ditado no Bloco de Notas, no Chrome e no VS Code; clipboard restaurado; overlay sem roubar foco). Se falhar de forma estrutural, reabrir este ADR com a Alternativa 2.
- **Fator ônibus e dependências git**. Mitigação: `cargo deny` com fontes git permitidas explicitamente e pinadas por `rev`; como o fork é divergente, a dependência do upstream é opcional.
- **Deriva de segurança** (hook global, clipboard). Mitigação: `ecc:security-reviewer` nos gatilhos já previstos (NFR-002-03) e revisão de `unsafe` (50 ocorrências) na T-001a.

## Portão da T-001 (Windows 11, 2026-09-30)

| Item | Resultado |
| --- | --- |
| Toolchain | Rust 1.98.1, Bun 1.4.2, CMake 4.4.3, VS Build Tools 2022 (MSVC 14.44), Vulkan SDK 1.4.357.0 |
| `cargo test` no código herdado (antes do rebranding) | **273 passando, 0 falhas** |
| `cargo test` depois do rebranding | **281 passando** (273 + testes novos de rótulos de janela e da política do updater) |
| Build nativo (`transcribe-cpp-sys` com Vulkan) | Falhou com `MSB1009`: o MSBuild da máquina não abre projetos através da junction em `%LOCALAPPDATA%\tcs` criada sem admin. Contornado com `scripts/windows-dev-env.ps1 -BypassJunction` (sem junction + `CARGO_TARGET_DIR` curto). Não é falha estrutural do fork; documentado no BUILD.md |
| Smoke manual (`tauri dev`, ditado no Bloco de Notas, Chrome e VS Code, clipboard restaurado, overlay sem roubar foco) | _pendente — preencher após o teste manual_ |

Nada até aqui pede reabrir esta decisão.
