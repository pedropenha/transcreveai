# T-090 — implementação e validação

Data: 2026-10-03. Checkout `C:\multimidia\ecc`, branch `integration/v1`.

## Resultado

Implementação e aceitação do escopo concluídas. Em 2026-10-03, o usuário autorizou explicitamente o commit apesar da falha anterior da catraca global fora da T-090. O checkbox foi concluído; a dívida continua registrada e a validação manual do app fica com o usuário. Alterações anteriores do assistente foram preservadas; não houve troca de branch, push, instalação ou envio de histórico real.

## Comportamento entregue

- Codex e Claude Code reutilizam o login do CLI, sem API key/keyring. O roteador trata esses providers sem custo de API e os serve ao resumo e ao assistente.
- Windows resolve o Codex npm para seu executável nativo em layouts conhecidos x64/ARM64; `.cmd`/`.bat` não são executados. Instalações incompatíveis são recusadas, e a busca continua para candidatos compatíveis posteriores.
- Codex ignora configuração pessoal/regras; Claude usa safe mode, ferramentas/MCP desativados e sessão sem persistência. Flags não reconhecidos falham sem remover proteções.
- Diretório privado vazio por chamada, ambiente por lista positiva, prompt via stdin, saída limitada e logs sem conteúdo de stderr. Timeout/cancel encerram árvore e tarefas de pipes.
- Argumentos extras são restritos a opções verificadas: Codex `--color=never|auto`, Claude `--effort=low|medium|high|xhigh|max`. Modelo tem campo próprio.
- Agentes ausentes/desabilitados não podem ser selecionados no assistente; configuração independente permite habilitar e informar caminho sem mudar o provider do resumo/assistente. Falhas localizadas preservam rascunhos.
- Cursor/Devin continuam experimentais; seleção explícita do assistente é necessária. Auto nunca os escolhe, nem pelo fallback de provider de resumo. Resumos silenciosos continuam recusados nesses adaptadores.

## Evidências finais

| Verificação                 | Resultado                                                                                                            |
| --------------------------- | -------------------------------------------------------------------------------------------------------------------- |
| TDD CLI                     | RED demonstrado; 47 testes focados GREEN                                                                             |
| Regressão Auto experimental | RED antes da correção; GREEN para Cursor/Devin e escolha explícita                                                   |
| Rust completo               | 957 passaram, zero falhas; 3 testes de rede opt-in separados                                                         |
| Aceitação real              | 3/3 passaram: resumo sintético Codex, resumo sintético Claude e encerramento do processo Codex                       |
| Rust formato/Clippy         | `cargo fmt --check` e `clippy --all-targets -- -D warnings` PASS                                                     |
| Frontend                    | Build, TypeScript, lint (zero erros; 15 avisos anteriores), traduções e testes unitários PASS                        |
| E2E do escopo               | 11/11 assistente/configuração CLI; 19/19 Configurações/Modelos na repetição integral                                 |
| Formato frontend/diff       | Prettier dos arquivos da tarefa e `git diff --check` PASS                                                            |
| Revisão                     | Segurança independente e revisão Rust/TypeScript: nenhum bloqueio concreto residual no escopo                        |
| Dependências                | `cargo deny check` PASS; `cargo audit` no diretório `src-tauri` PASS com exceções herdadas já existentes e 13 avisos |

A primeira suíte Rust integral encontrou duas fixtures incompatíveis com a nova política (batch e `--search`); foram corrigidas preservando os critérios de privacidade/normalização. Clippy no build normal revelou `tempfile` apenas como dev-dependency: o crate já travado foi promovido para dependência de produção, sem mudar o lockfile. Helper/import usados só em testes foram marcados `cfg(test)`; os checks finais passaram.

A primeira regressão Configurações/Modelos teve 18 PASS e um timeout em clique de navegação. A repetição integral, com um worker e sem mudar teste/produto, passou 19/19. Isso não representa execução de toda a suíte Playwright nem comprova ausência geral de intermitência.

## Cobertura

Catraca de cobertura frontend PASS: 21 testes, 82,37% funções e 90,87%
linhas no conjunto instrumentado. A integração inicial dos novos testes Bun
no agregador fazia importação de suites dentro de callbacks de teste; foram
movidas para imports no topo do módulo. `check-frontend-coverage` e
`test:coverage` passaram após a correção.

LLVM, testes CLI focados, cobertura de linhas por arquivo: `cli_agent.rs` 90,24%; `adapters.rs` 92,31%; `parse.rs` 80,54%; `spawn.rs` 84,03%; `validate.rs` 93,10%. Métricas do escopo, não cobertura global do app.
Exportação preservada em `coverage/t090-rust.json` (ignorada pelo Git);
dados LLVM em `D:\t\llvm-cov-target\src-tauri.profdata`. Após a medição houve
somente ajustes de import/helper test-only e manifesto/fixtures, sem mudança
da lógica CLI medida. Reprodução e comando no handoff do backend.

Chromium V8 remapeado para fontes atuais, sem excluir callbacks e com hash das fontes: AssistantProvider 100% linhas/funções, 83,33% branches; CliAgentConfiguration 100% linhas/funções, 93,75% branches; CliAgentFields 100% linhas/funções, 86,36% branches. Statements também passam 80%. Helper de seleção tem 100% linhas/funções em Bun. Gate: `bun scripts/check-cli-settings-coverage.ts`; relatórios em `coverage/cli-settings/latest/` ignorados pelo Git.

## Bloqueio de fechamento

`bun scripts/check-inherited-debt.ts` falha: 16 arquivos Rust acima de 800 linhas (limite 12), 84 blocos unsafe (limite 63). `unwrap`/`expect` ficam em 152 (limite 158). A T-090 não acrescenta arquivo de produção acima de 800 linhas, unsafe ou panic de produção; o maior CLI alterado permanece abaixo do limite. Os tetos não foram elevados nem checks desativados. O usuário autorizou o commit deste escopo; a reconciliação da dívida fora da tarefa continua pendente.
Uma reconstrução isolada das fontes iniciais (revertendo apenas arquivos da
T-090 e restaurando snapshots anteriores dos arquivos compartilhados) confirmou
16/84/152 antes e depois. Resultado da baseline em
`C:\Users\pedro\AppData\Local\Temp\transcreve-t090-baseline\debt-baseline.log`.

`cargo audit` executado inicialmente da raiz não carregou `src-tauri/.cargo/audit.toml` e informou vulnerabilidades herdadas; a execução final no diretório configurado, como no CI, passou. A lista de exceções não foi alterada.

## Limites da aceitação

Versões reais: Windows, Codex 0.159.3 e Claude Code 2.1.273. Chamadas usaram somente reunião fictícia, sem acesso ao histórico do app. Resumos foram exercitados pelo contrato `LlmProvider` com propósito Summary; não foi criado um registro de reunião na UI. Cancelamento real confirma processo vivo e depois encerrado; não confirma streaming remoto, Esc/botão do painel ou ditado real. E2E frontend usa IPC simulado. macOS/Linux, Cursor/Devin e instalador não foram validados nesta tarefa; não há garantia de modo não-mutante dos experimentais.

## Coordenação

Skill: [team-agent-orchestration](C:/Users/pedro/.codex/plugins/cache/ecc/ecc/2.2.3/skills/team-agent-orchestration/SKILL.md).

Quadro: [t090-orchestration.md](t090-orchestration.md). Handoffs: [UI](t090-ui-handoff.md), [backend](t090-backend-handoff.md), [segurança](t090-security-review.md). Integração e commit no checkout existente, somente do escopo T-090; nenhuma branch nova ou merge Git. Nenhuma skill compartilhada criada: o padrão já está coberto pela skill invocada.
