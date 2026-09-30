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

Fazemos **fork do Handy** (`29bd2c0`) como base, renomeado para **Transcreve.ai**, em modo **divergente**. Mantemos um remote `upstream` e fazemos **cherry-pick seletivo** das correções nas áreas ainda compartilhadas (áudio, `paste_tx`, catálogo de modelos, bumps de transcribe-cpp/transcribe-rs, correções de Windows), revisando as releases do Handy uma vez por mês. As ECC rules valem integralmente para código **novo ou alterado**. Para o código herdado, uma tarefa de **baseline** (nova T-001a) mede cobertura, configura as ferramentas e registra as exceções; a T-002 automatiza as catracas na CI. Um arquivo herdado com mais de 800 linhas é dividido quando for alterado, e a cobertura total sobe em catraca até ≥ 80%.

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

| Item                                                                                                                  | Resultado                                                                                                                                                                                                                                                                                  |
| --------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| Toolchain                                                                                                             | Rust 1.98.1, Bun 1.4.2, CMake 4.4.3, VS Build Tools 2022 (MSVC 14.44), Vulkan SDK 1.4.357.0                                                                                                                                                                                                |
| `cargo test` no código herdado (antes do rebranding)                                                                  | **273 passando, 0 falhas**                                                                                                                                                                                                                                                                 |
| `cargo test` depois do rebranding                                                                                     | **281 passando** (273 + testes novos de rótulos de janela e da política do updater)                                                                                                                                                                                                        |
| Build nativo (`transcribe-cpp-sys` com Vulkan)                                                                        | Falhou com `MSB1009`: o MSBuild da máquina não abre projetos através da junction em `%LOCALAPPDATA%\tcs` criada sem admin. Contornado com `scripts/windows-dev-env.ps1 -BypassJunction` (sem junction + `CARGO_TARGET_DIR` curto). Não é falha estrutural do fork; documentado no BUILD.md |
| Smoke manual (`tauri dev`, ditado no Bloco de Notas, Chrome e VS Code, clipboard restaurado, overlay sem roubar foco) | **Aprovado**                                                                                                                                                                                                                                                                               |

Nada até aqui pede reabrir esta decisão.

## Exceções herdadas e baseline ECC (T-001a)

Baseline medida em 2026-09-30 no Windows 11, na branch `chore/t-001a-baseline`, no estado final de código da tarefa. Nenhum relatório HTML, LCOV, `target/` ou outro artefato de cobertura é versionado.

### Ferramentas e comandos

| Ferramenta       | Versão                           |
| ---------------- | -------------------------------- |
| `rustc`          | 1.98.1 (`48a229cea`, 2026-09-01) |
| `cargo`          | 1.98.1 (`797e8a9bc`, 2026-08-05) |
| Bun              | 1.4.2 (`744846f84`)              |
| `cargo-llvm-cov` | 0.9.1                            |
| `cargo-deny`     | 0.20.2                           |
| `cargo-audit`    | 0.22.2                           |

`cargo-llvm-cov`, `cargo-deny` e `cargo-audit` não estavam instalados e foram instalados com `cargo install --locked` nas versões acima. O `cargo-llvm-cov` instalou automaticamente o componente `llvm-tools-preview` da toolchain estável já ativa; a versão da toolchain não mudou.

Os comandos de medição foram:

```powershell
cd C:\multimidia\ecc
$env:Path = "$HOME\.cargo\bin;$env:Path"
. .\scripts\windows-dev-env.ps1 -BypassJunction
bun run test:coverage

cd C:\multimidia\ecc\src-tauri
$env:Path = "C:\Program Files\CMake\bin;$HOME\.cargo\bin;$env:Path"
cargo llvm-cov --workspace --all-targets
cargo clippy --workspace --all-targets -- -D warnings
cargo deny check
cargo audit
```

### Cobertura medida

O backend executou **281 testes, 0 falhas**. O resumo do `cargo llvm-cov` foi:

| Métrica Rust |                 Cobertos / total |  Cobertura |
| ------------ | -------------------------------: | ---------: |
| Regiões      |                   8.710 / 21.854 | **39,86%** |
| Funções      |                      642 / 1.554 | **41,31%** |
| Linhas       |                   5.588 / 14.919 | **37,46%** |
| Branches     | indisponível na execução estável |          — |

Arquivos representativos mostram a distribuição desigual da cobertura herdada:

| Arquivo/área                           | Linhas |
| -------------------------------------- | -----: |
| `audio_toolkit/text.rs`                | 99,11% |
| `audio_toolkit/audio/resampler.rs`     | 95,41% |
| `transcription_coordinator.rs`         | 84,33% |
| `settings.rs`                          | 79,73% |
| `managers/model.rs`                    | 31,03% |
| `managers/transcription.rs`            | 23,06% |
| `clipboard.rs`                         | 14,85% |
| `lib.rs`                               |  3,11% |
| `paste_tx/windows.rs` e comandos Tauri |  0,00% |

Limitações: a medição foi feita no Windows e não compila/exercita os caminhos `cfg(target_os = "macos")` e `cfg(target_os = "linux")`; o código C/C++ de `transcribe-cpp` não entra na cobertura Rust; branches e doctests instrumentados exigiriam configuração adicional/nightly.

O frontend usa os quatro testes standalone já existentes, agora registrados por um wrapper mínimo no runner do Bun: **4 testes, 0 falhas**, com 23 pontos de asserção no código-fonte. O resultado foi:

| Arquivo carregado                                    |    Funções |     Linhas |
| ---------------------------------------------------- | ---------: | ---------: |
| `src/components/icons/soundBars.ts`                  |    100,00% |    100,00% |
| `src/components/settings/history/clipboard.ts`       |    100,00% |    100,00% |
| `src/components/update-checker/portableInstaller.ts` |    100,00% |    100,00% |
| `src/lib/utils/keyboard.ts`                          |     71,43% |     86,45% |
| **Total dos arquivos carregados**                    | **92,86%** | **96,61%** |

Essa porcentagem **não é cobertura global do frontend**: o Bun só inclui os quatro módulos importados pelos testes e não instrumenta os demais componentes React. Playwright também não participa desta baseline. A T-002 deve manter a mesma seleção para comparação até ampliar o conjunto de testes, sem interpretar 96,61% como cobertura de `src/` inteiro.

### Clippy

`cargo clippy --workspace --all-targets -- -D warnings` passa sem warnings do compilador. Foram corrigidos 21 diagnósticos herdados com mudanças mecânicas e sem alteração intencional de produto: imports condicionais, branches idênticos, `return` redundante, referências redundantes em `format!`, comparação de ponteiros, `repeat_n`, `writeln!` e escopo de variável.

Há duas exceções estreitas, em `src-tauri/src/lib.rs:417` e `src-tauri/src/managers/transcription.rs:2159`: `#[allow(clippy::items_after_test_module)]` aplicado somente aos módulos de teste. Mover esses módulos para o fim de arquivos herdados de 1.125 e 2.529 linhas aumentaria o churn sem benefício comportamental nesta baseline; a exceção deve desaparecer quando cada arquivo for dividido pela tarefa funcional que o tocar.

O Clippy foi executado no Windows e não compila trechos exclusivos de macOS/Linux; a T-002 deve repetir o gate nos runners de cada plataforma. A alteração em `helpers/clamshell.rs` foi também revisada estaticamente, mas só será compilada no macOS.

### Fontes, licenças, bans e advisories

`cargo deny check` passa para Windows x64/ARM64, Linux x64/ARM64 e macOS x64/ARM64. A política nega registry e Git desconhecidos, permite explicitamente somente crates.io e as fontes abaixo, e exige `rev` para toda dependência Git:

| Fonte Git                           | `rev` fixado                               |
| ----------------------------------- | ------------------------------------------ |
| `rustdesk-org/rdev`                 | `a90dbe1172f8832f54c97c62e823c5a34af5fdfe` |
| `cjpais/vad-rs`                     | `2a412ed858695b9251f3f5a1a20d95b59fa7c498` |
| `cjpais/rodio`                      | `fed30292db417cb95305c118c0e1d804fb74cbff` |
| `cjpais/hf-hub`                     | `9918f11ab3473135eb7865aa4a5d79d597e6e810` |
| `cjpais/tao` (`tao` e `tao-macros`) | `c3bee28c1d446d95f08c95c3b6f8d4bde052b876` |
| `ahkohd/tauri-nspanel`              | `da9c9a8d4eb7f0524a2508988df1a7d9585b4904` |

A licença permitida é uma allowlist de licenças permissivas/compatíveis (`MIT`, Apache, BSD, ISC, MPL-2.0, Zlib e equivalentes encontrados). `tauri-nspanel` mantém um aviso herdado por não declarar `license` no manifesto, embora o repositório distribua arquivos MIT/Apache-2.0. Bans ficam em catraca com **64 famílias de versões duplicadas** e **5 dependências Git sem versão semver**; estas últimas são aceitas como wildcard porque a proteção efetiva é `required-git-spec = "rev"`.

A primeira auditoria encontrou 11 vulnerabilidades e 19 avisos. Atualizações compatíveis no lock corrigiram `anyhow`, `event-listener`, `h2`, `memmap2`, `rand` 0.8, `rustls`, `rustls-webpki`, `tar` e a família `wasm-bindgen/js-sys`; `cargo audit` agora passa com 3 vulnerabilidades herdadas explicitamente aceitas e 13 avisos informativos:

| Advisory            | Crate              | Severidade/alcance                          | Justificativa e plano                                                                                                                                                          |
| ------------------- | ------------------ | ------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `RUSTSEC-2026-0194` | `quick-xml 0.38.4` | 7,5 alta; parser de XML                     | Dependência de build via `plist`/Tauri e `wayland-scanner`; `plist 1.8.0` exige `quick-xml ^0.38`. Reavaliar assim que os upstreams aceitarem `>=0.41`.                        |
| `RUSTSEC-2026-0195` | `quick-xml 0.38.4` | 7,5 alta; DoS por namespace                 | Mesmos caminhos exclusivamente de build e mesmo plano do advisory anterior.                                                                                                    |
| `RUSTSEC-2026-0235` | `rkyv 0.7.46`      | leitura fora de limites, sem CVSS publicado | Feature opcional inativa presente no lock via `rust_decimal`; não aparece no grafo resolvido do `cargo-deny`. Remover/atualizar quando o upstream eliminar esse ramo opcional. |

Os 13 avisos informativos aceitos são: `RUSTSEC-2025-0141`, `RUSTSEC-2025-0057`, `RUSTSEC-2024-0422`, `RUSTSEC-2024-0423`, `RUSTSEC-2024-0436`, `RUSTSEC-2024-0370`, `RUSTSEC-2025-0081`, `RUSTSEC-2025-0075`, `RUSTSEC-2025-0080`, `RUSTSEC-2025-0100`, `RUSTSEC-2025-0098`, `RUSTSEC-2024-0429` e `RUSTSEC-2026-0097`. São dependências transitivas sem manutenção, GTK3 exclusivo do Linux ou casos de unsoundness em APIs não usadas. Devem ser reavaliados em todo upgrade de dependências; os avisos GTK3/glib têm como limite a T-084 (port Linux).

### Inventário de dívida herdada

Os números da T-000 eram estimativas por leitura antes do rebranding. A T-001a usa contagem automatizada no estado atual e inclui `expect()` junto de `unwrap()`, por isso substitui os valores antigos para fins de catraca.

Há **12 arquivos Rust acima de 800 linhas**:

| Arquivo                           | Linhas | Classificação                                                           |
| --------------------------------- | -----: | ----------------------------------------------------------------------- |
| `managers/model.rs`               |  3.090 | aceita temporariamente; dividir na próxima tarefa funcional que o tocar |
| `managers/transcription.rs`       |  2.529 | aceita temporariamente; T-012 deve dividir por domínio                  |
| `settings.rs`                     |  1.738 | aceita temporariamente; dividir na próxima tarefa funcional que o tocar |
| `transcription_coordinator.rs`    |  1.617 | aceita temporariamente; dividir na próxima tarefa funcional que o tocar |
| `shortcut/mod.rs`                 |  1.427 | aceita temporariamente; dividir na próxima tarefa funcional que o tocar |
| `lib.rs`                          |  1.125 | aceita temporariamente; dividir na próxima tarefa funcional que o tocar |
| `managers/audio.rs`               |  1.101 | aceita temporariamente; dividir na próxima tarefa funcional que o tocar |
| `actions.rs`                      |  1.053 | aceita temporariamente; dividir na próxima tarefa funcional que o tocar |
| `audio_toolkit/audio/recorder.rs` |  1.046 | aceita temporariamente; dividir na próxima tarefa funcional que o tocar |
| `clipboard.rs`                    |  1.015 | aceita temporariamente; dividir na próxima tarefa funcional que o tocar |
| `overlay.rs`                      |    895 | aceita temporariamente; dividir na próxima tarefa funcional que o tocar |
| `audio_toolkit/text.rs`           |    884 | aceita temporariamente; dividir na próxima tarefa funcional que o tocar |

As correções mecânicas de Clippy em `model.rs`, `transcription.rs` e `lib.rs` não iniciaram refactor estrutural porque a própria T-001a proíbe refactor massivo. Isso não cria precedente para tarefas funcionais.

O inventário de produção, medido com `clippy::unwrap_used`/`clippy::expect_used` sem `cfg(test)` e sem `build.rs`, contém **146 ocorrências: 122 `unwrap()` e 24 `expect()`**:

| Arquivo                            | `unwrap` | `expect` | Total |
| ---------------------------------- | -------: | -------: | ----: |
| `audio_toolkit/audio/recorder.rs`  |        7 |        4 |    11 |
| `audio_toolkit/audio/resampler.rs` |        4 |        1 |     5 |
| `audio_toolkit/text.rs`            |        2 |        0 |     2 |
| `catalog/mod.rs`                   |        0 |        1 |     1 |
| `managers/audio.rs`                |       45 |        0 |    45 |
| `managers/history.rs`              |        0 |        1 |     1 |
| `managers/model.rs`                |       32 |        0 |    32 |
| `managers/transcription.rs`        |       18 |        1 |    19 |
| `paste_tx/windows.rs`              |        6 |        0 |     6 |
| `settings.rs`                      |        4 |        5 |     9 |
| `shortcut/mod.rs`                  |        0 |        2 |     2 |
| `tray_i18n.rs`                     |        0 |        1 |     1 |
| `lib.rs`                           |        4 |        8 |    12 |

Essas ocorrências são exceções herdadas temporárias; nenhuma foi removida em massa nesta tarefa. A próxima tarefa que tocar um desses trechos deve substituir os panics por propagação/recuperação adequada, e a contagem total não pode aumentar.

### Revisão dos `unsafe`

O `ecc:security-reviewer` revisou as **50 ocorrências textuais de produção**. Todas são FFI, syscall ou integração de SO necessária; nenhuma deve ser removida apenas para reduzir a contagem. A documentação de invariantes está adequada em 12, parcial em 7 e ausente em 31.

| Grupo                         | Ocorrências | Risco e classificação                                   | Ação                                                                                                                                                       |
| ----------------------------- | ----------: | ------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `paste_tx/windows.rs:89-495`  |          16 | médio; requer revisão antes de tocar                    | Zerar `GWLP_USERDATA` antes de `Arc::from_raw`, evitar panic no `wnd_proc`, excluir formatos GDI não-`HGLOBAL`, liberar `HBITMAP` e documentar `# Safety`. |
| `paste_tx/macos.rs:50-293`    |           6 | médio no owner prometido; requer revisão antes de tocar | Manter o provider vivo se o restore falhar; documentar selectors e `msg_send!`.                                                                            |
| `apple_intelligence.rs:20-70` |           6 | médio; corrigir na próxima tarefa do arquivo            | Guard RAII para `free_apple_llm_response`, FFI privada, comentários SAFETY e timeout separado no Swift.                                                    |
| `input.rs:27-111`             |           8 | baixo; seis blocos já adequados                         | Preservar Create/Get Rule de CoreFoundation; documentar assinaturas `extern` e validar main thread ao tocar.                                               |
| `autostart.rs:64-83`          |           4 | baixo; aceita temporariamente                           | Documentar o guard de disponibilidade de `SMAppService`.                                                                                                   |
| `overlay.rs:162,363`          |           2 | baixo; aceita temporariamente                           | Documentar validade do `HWND` e thread principal.                                                                                                          |
| `managers/audio.rs:33,120`    |           2 | baixo de memória/médio de ciclo COM                     | Balancear `CoInitializeEx`/`CoUninitialize`, reduzir locks durante COM e documentar SAFETY na próxima tarefa do arquivo.                                   |
| `utils.rs:61,66`              |           2 | baixo; já adequada                                      | Manter resolução dinâmica e assinatura de `IsWow64Process2`.                                                                                               |
| `memory.rs:32,49`             |           2 | baixo; já adequada                                      | Manter comentários SAFETY das chamadas libc sem ponteiros de entrada.                                                                                      |
| `lib.rs:971`                  |           1 | baixo; aceita temporariamente                           | Encolher o bloco e documentar thread/refcount COM ao tocar WebView2.                                                                                       |
| `secure_input.rs:146`         |           1 | baixo; aceita temporariamente                           | Documentar getter FFI e bloco `extern`.                                                                                                                    |

Os riscos médios de clipboard são dívida concreta de segurança, mas corrigi-los nesta baseline mudaria comportamento de produto e exigiria testes específicos por plataforma. Eles devem ser tratados antes ou dentro da próxima tarefa que tocar cada implementação.

### Fins de linha e catraca para T-002

Com `core.autocrlf=true`, o checkout inicial tinha 162 arquivos de texto em CRLF embora o índice já estivesse em LF. `.gitattributes` agora força `* text=auto eol=lf` e marca PNG, ICO, ICNS, WAV e ONNX como binários. Não houve normalização histórica de conteúdo para separar: o índice já estava em LF.

A T-002 deve automatizar estas catracas, sem alterar os números desta baseline silenciosamente:

1. Cobertura Rust no mesmo alvo/comando não pode ficar abaixo de **37,46% linhas**, **41,31% funções** ou **39,86% regiões**; cada alvo adicional recebe sua própria baseline. Código novo ou alterado continua sujeito a >= 80%.
2. Os quatro módulos frontend carregados não podem cair abaixo de **96,61% linhas** e **92,86% funções**; ampliar o conjunto é permitido, reduzir ou omitir testes não. Não tratar essa métrica como cobertura global de `src/`.
3. Clippy permanece com **zero warnings** em `--workspace --all-targets -- -D warnings`; não ampliar os dois `allow` existentes.
4. Nenhuma fonte Git fora da allowlist e nenhum Git sem `rev`; nenhuma licença fora da allowlist; famílias duplicadas <= 64 e wildcards Git <= 5.
5. Nenhum advisory novo sem exceção com motivo, alcance e plano registrados; as três vulnerabilidades e os 13 avisos aceitos só podem diminuir.
6. Arquivos Rust acima de 800 linhas <= 12, `unwrap`/`expect` de produção <= 146 e `unsafe` de produção <= 50. Novo `unsafe` exige comentário SAFETY e `ecc:security-reviewer`; a cobertura de documentação SAFETY não pode cair abaixo de 12 adequadas.
