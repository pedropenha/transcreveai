# Como contribuir

Obrigado pelo interesse no Transcreve.ai! O projeto é guiado por especificações: antes de escrever código, leia a [constituição](specs/constitution.md) e a spec da feature em [`specs/features/`](specs/features/).

## Fluxo

1. **Escolha uma tarefa** em [`specs/tasks.md`](specs/tasks.md) (`T-0xx`). Cada tarefa aponta para os requisitos (`FR-*`) e critérios de aceite (`AC-*`) que ela atende.
2. **Siga o fluxo da tarefa** descrito no topo do `tasks.md`: pesquisa/reuso, plano, TDD (os `AC-*` viram testes), revisão e verificação.
3. **Mudou o comportamento do produto?** Atualize a spec da feature no mesmo PR.
4. **Decisão de arquitetura?** Registre um ADR em [`docs/adr/`](docs/adr/).

## Ambiente

Pré-requisitos e comandos estão no [README](README.md#pré-requisitos-no-windows) e no [BUILD.md](BUILD.md). No Windows, compile pelo PowerShell depois de `. .\scripts\windows-dev-env.ps1 -BypassJunction`.

## Antes de abrir o PR

- `bun run lint`, `bun run format:check`, `bun run check:translations`, `bun run test:unit`
- `cargo fmt --check`, `cargo test` e `cargo clippy` em `src-tauri/`
- Textos visíveis ao usuário passam pelo i18n (`src/i18n/locales/en` e `pt`); ver [CONTRIBUTING_TRANSLATIONS.md](CONTRIBUTING_TRANSLATIONS.md).
- Commits convencionais (`feat:`, `fix:`, `docs:`, `refactor:`, `chore:`), com a mensagem explicando o porquê.

## Correções vindas do Handy

O remote `upstream` aponta para o [Handy](https://github.com/cjpais/Handy), com push bloqueado. Correções do Handy entram por **cherry-pick seletivo** (ver [ADR-0001](docs/adr/0001-fork-do-handy-como-base.md)); cite o commit de origem na mensagem. Bugs que também existem no Handy devem ser reportados lá.
