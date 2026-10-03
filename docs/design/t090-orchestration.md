# T-090 — quadro de execução

Data: 2026-10-03. Integrador: Codex principal. Checkout: `C:\multimidia\ecc`, branch `integration/v1`.

Execução no checkout atual com propriedade exclusiva por arquivo; sem trocar branches, descartar alterações existentes ou misturar a entrega anterior do assistente. Os arquivos compartilhados de spec/traduções serão integrados preservando o conteúdo anterior.

| Card                   | Owner           | Estado                        | Escopo                                            | Gate                                                                  |
| ---------------------- | --------------- | ----------------------------- | ------------------------------------------------- | --------------------------------------------------------------------- |
| Configuração CLI       | t090_ui         | Review — gates do escopo PASS | Componentes de configuração e testes próprios     | TDD, typecheck, lint, E2E, cobertura e revisão                        |
| Execução CLI           | t090_backend    | Review — gates do escopo PASS | `src-tauri/src/llm/cli_agent*`, fixtures IPC      | TDD, argv sem shell, configuração isolada, timeout/cancel e cobertura |
| Segurança              | t090_security   | Review — aprovado             | Revisão independente, sem editar código           | Nenhum HIGH/CRITICAL aberto                                           |
| Aceitação e integração | Codex principal | Merged — commit autorizado    | Spec, task, relatório e chamadas sintéticas reais | Escopo PASS; dívida anterior registrada, commit autorizado            |

## Plano e critérios

1. Corrigir seleção/configuração sem permitir resumo silencioso por adaptador experimental.
2. Resolver shims npm Windows pelo runtime e entrypoint conhecidos, recusando scripts arbitrários; nunca passar prompt por argv ou arquivo.
3. Isolar configuração/hooks/MCP mantendo autenticação do CLI e limitar argumentos extras a opções seguras verificadas.
4. Executar testes unitários, integração de processos e chamadas autenticadas com conteúdo fictício; preservar configurações e histórico do app.
5. Revisar segurança e mudanças finais; atualizar T-090 somente conforme evidência. Commit apenas do escopo desta tarefa quando os gates passarem.

Pesquisa: padrões existentes do projeto reutilizados; GitHub CLI indisponível sem autenticação. Ajuda das versões instaladas e documentação oficial usadas como fontes primárias.

Handoffs: `t090-ui-handoff.md`, `t090-backend-handoff.md`, `t090-security-review.md`; resultado consolidado em `t090-validation.md`.

## Pass final

957 testes Rust e três testes reais opt-in passaram. Build, lint, traduções,
formato, Clippy, dependências configuradas e 30 cenários E2E do escopo/regressão
passaram. Cobertura CLI/UI cumpre 80% nos critérios medidos; revisões aprovadas.

Bloqueio do integrador: catraca global 16 arquivos grandes/84 unsafe contra
limites 12/63. Reconstrução isolada do estado inicial confirmou exatamente os
mesmos números, inclusive 152 unwrap/expect; nenhuma regressão desta tarefa.
O usuário autorizou explicitamente o commit da T-090; reconciliar a dívida existente continua como trabalho separado. Não foram elevados
limites ou descartadas alterações anteriores. Commit somente do escopo T-090, sem push ou merge Git;
integração disponível no working tree. Nenhuma nova skill compartilhada necessária.
