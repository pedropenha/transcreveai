# Correções retomadas do Claude

Data: 2026-10-05. Checkout `C:\multimidia\ecc`, branch `integration/v1`.
Contexto recuperado da sessão Claude `f9fd0728-88bc-49fa-a3a0-716dc56d3020` e do worktree `agent-a7c7fdf91fdaaf521`. Alterações anteriores do assistente, tradução por sessão e prévia visual foram preservadas. Sem commits ou instalação nesta retomada.

## Resultado

- **Origem dos novos ditados:** captura do aplicativo em foco antes de abrir o microfone, transferida para a sessão ao parar. Nome e executável aparecem no histórico; o caminho sanitizado fica no backend e permite extrair o ícone no Windows. Chrome continua Chrome, independentemente da página. Registros antigos sem origem continuam desconhecidos; não há inferência retroativa.
- **Persistência:** migração 16 aditiva, com teste de preservação dos dados e da busca FTS. O caminho não é serializado por IPC. Cancelamento/falha limpa a origem estacionada; pipelines já enfileirados mantêm sua própria origem.
- **Ícones:** logo embutido, ícone extraído, monograma ou indicação neutra. Consulta preguiçosa, compartilhada entre linha e detalhe. Cache por entrada evita que registros antigos sem caminho suprimam ícones novos; limite FIFO de 128 respostas evita retenção ilimitada. Falhas de IPC permitem nova tentativa. Nome em branco usa o executável como alternativa.
- **Tradução:** preservados aviso com ícone de atenção, indicação de download, explicação dos modelos compatíveis e selo na tabela. Erros conhecidos de tradução deixam de gerar um segundo aviso Sonner no Hub, pois já possuem aviso nativo não ativante. Erros comuns de microfone continuam no Hub.
- **Avisos flutuantes:** preservada a implementação herdada do cartão de reunião e da mensagem completa de tradução; concluídos testes de geometria, teclado e acessibilidade. Detalhes em [toast-resumed-validation.md](toast-resumed-validation.md).
- **Suporte aos testes release:** os auxiliares de memória e exportação de bindings também compilam em `cfg(test)`. A seleção de credenciais em memória em execução continua estritamente restrita a debug; o aplicativo release usa o cofre do sistema. Removido helper de estado de carga sem consumidores.

## Evidências de regressão

| Comportamento                                          | Evidência                                                                                                                                                               |
| ------------------------------------------------------ | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Erro traduzido não aparece duas vezes no Hub           | Playwright falhou com dois avisos antes da alteração de `App.tsx`; passou depois. Controle positivo mantém o aviso de microfone.                                        |
| Registro antigo não bloqueia o ícone de outro registro | Regressão RED → GREEN no helper e teste de histórico com dois registros do mesmo executável.                                                                            |
| Nome amigável em branco usa executável                 | Regressões RED → GREEN no helper frontend e no helper Rust; teste backend também incluído na suíte.                                                                     |
| Cache limitado sem perder deduplicação                 | Teste unitário RED → GREEN com 129 entradas, descarte e nova consulta. Incluído isoladamente em `test:unit` para não compartilhar o mock de bindings com outros testes. |
| Testes em release compilam seus auxiliares             | Primeira compilação falhou com `E0433` em `MemorySecretStore`, `Typescript` e `BigIntExportBehavior`; condições de compilação corrigidas.                               |

## Verificação automatizada

- `bun run build`, `bun run lint`, `bun run check:translations`, `bun run test:unit` e `bun run test:coverage` passaram. Lint sem erros, com avisos preexistentes.
- Suíte Playwright integrada: **183 aprovados, 84 capturas opcionais não executadas**, antes da inclusão dos dois últimos cenários de cache/detalhe. Os cenários adicionais foram verificados na execução focada final.
- Histórico final: **14 testes aprovados**. Tradução/modelos: **17 testes aprovados**; tradução com cobertura final: **13 testes aprovados**. Avisos: **20 testes aprovados**, incluindo axe sem violações nas condições verificadas.
- Cobertura Chromium remapeada por sourcemaps, com hashes das fontes atuais e sem exclusões:

| Fonte                             | Linhas/declarações | Funções | Ramificações |
| --------------------------------- | ------------------ | ------- | ------------ |
| `HistoryAppLogo.tsx`              | 100%               | 100%    | 100%         |
| `historyIcons.ts`                 | 95,55%             | 100%    | 81,81%       |
| `useSeen.ts`                      | 100%               | 100%    | 100%         |
| `homeView.ts`                     | 94,66%             | 91,66%  | 91,42%       |
| `TranslatedDictationSettings.tsx` | 99,39%             | 100%    | 93,93%       |
| `translationModels.ts`            | 100%               | 100%    | 100%         |

Relatórios em `coverage/corrections/origin-pass2/coverage-final.json` e `coverage/corrections/translation-20261005/coverage-final.json`; os caminhos de descarte do cache também possuem teste unitário.

- Revisão independente de código e segurança: sem CRITICAL/HIGH no escopo retomado. O apontamento de cache ilimitado foi corrigido e revisado novamente.
- `cargo llvm-cov --lib --json --summary-only`: **998 testes aprovados, 3 ignorados**, incluindo igualdade literal das bindings, preservação da migração/FTS e extração real do ícone do Bloco de Notas no Windows.
- Testes release: compilação concluída. A primeira execução concorrente aprovou 997 casos e falhou na extração real do ícone do Bloco de Notas, cujo orçamento é de dois segundos. Reexecução integral do mesmo binário otimizado com `--test-threads=1`: **998 aprovados, 3 ignorados**, sem enfraquecer as asserções ou alterar o limite de produção.
- `cargo clippy --release --lib --tests -- -D warnings`: passou sobre o código final. O binário release da reexecução antecede apenas a remoção do método sem consumidores e a atualização de seu comentário; a suíte instrumentada e o Clippy validaram as fontes finais.
- Probe adicional do executável Claude instalado em `WindowsApps`: caminho aceito e ícone extraído com sucesso (`safe=true extracted=true`). Não inicia gravação nem modifica o app ou suas configurações.
- Cobertura LLVM: `dictation_origin/mod.rs` 98,59% linhas / 94,12% funções / 98,41% regiões; `dictation_origin/win.rs` 88,89% / 100% / 88,64%; `meeting/app_icon/mod.rs` 88,46% / 82,76% / 88,26%; `meeting/app_icon/win.rs` 93,23% / 100% / 87,50%. A política `translation.rs` ficou em 100%. Estes números incluem testes presentes nos módulos; não medem todo o fluxo real de ditado.
- `db/dictations.rs`: 97,79% linhas / 94,23% funções / 94,66% regiões. O arquivo herdado `commands/history.rs` permanece com 22,28% linhas e 17,5% funções: testes exercitam a política de seleção de ícones e privacidade, mas não os wrappers IPC reais. Cobertura global da biblioteca: 53% linhas / 53,43% funções / 54,55% regiões, sem alegar 80% global nem substituição da catraca `--workspace --all-targets`.
- Formatação Rust passou. `git diff --check` completo mantém os dois espaços finais herdados do gerador em `src/bindings.ts`; igualdade literal com o exporter é verificada pelo teste de bindings.

## Limites

Antes do commit, a revisão independente foi ampliada para todo o conjunto pendente, incluindo assistente, compositor, janela nativa, tradução e inserção protegida. Nenhum CRITICAL/HIGH ou bloqueio de segurança foi identificado. A busca de padrões de segredos nos arquivos de texto pendentes não encontrou candidatos. Essa revisão não substitui o aceite nativo descrito abaixo.

O app debug em execução foi preservado. Ele bloqueia cópia de DLLs da compilação debug; as verificações backend usaram artefatos release e instrumentados separados. O processo já aberto continua com o backend anterior até ser reiniciado; não houve teste real das novas correções de backend nesse processo.

Os testes de navegador usam IPC Tauri simulado. Não comprovam ditado real, tradução de fala, foco/clipboard entre aplicativos, DPI real WebView2, instalação NSIS ou macOS/Linux. A captura da origem e extração nativa nova são Windows somente. A T-093 conserva seu aceite nativo pendente; este relatório não encerra as pendências anteriores do assistente.
