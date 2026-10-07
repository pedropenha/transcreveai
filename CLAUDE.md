# CLAUDE.md

Transcreve.ai é um app de ditado por voz (e notas de reunião) que replica o comportamento do Wispr Flow, com transcrição local ou via API com chave própria. É um fork divergente do [Handy](https://github.com/cjpais/Handy) (MIT) — ver [ADR-0001](docs/adr/0001-fork-do-handy-como-base.md).

## Hierarquia de autoridade

Siga a [constituição](specs/constitution.md) §0:

1. **ECC rules e skills** mandam em tudo que é engenharia (processo, TDD, cobertura, revisão, estilo, segurança, git).
2. **Constituição e specs** (`specs/`) mandam no produto: o que o app faz e como se comporta.
3. Conflito de engenharia entre spec e rule: a rule vence e a spec é corrigida no mesmo PR.

## Onde está cada coisa

- [`specs/README.md`](specs/README.md) — índice das specs (produto, arquitetura, features `F0xx`).
- [`specs/tasks.md`](specs/tasks.md) — plano de tarefas (`T-0xx`) e o fluxo de cada tarefa.
- [`docs/adr/`](docs/adr/) — decisões de arquitetura.
- [`AGENTS.md`](AGENTS.md) — comandos de desenvolvimento e visão da arquitetura do código herdado.
- [`BUILD.md`](BUILD.md) — pré-requisitos e build por plataforma.

## Build no Windows

- Rode `cargo` e `bun run tauri …` no **PowerShell**, não no Git Bash.
- Antes de compilar, carregue o ambiente com `. .\scripts\windows-dev-env.ps1 -BypassJunction` (define `VULKAN_SDK`, um `CARGO_TARGET_DIR` curto e desvia da junction do `transcribe-cpp-sys`, que o MSBuild desta máquina não segue — erro `MSB1009`). Detalhes em [BUILD.md](BUILD.md#windows).

## Git

- Remote `origin` = [pedropenha/transcreveai](https://github.com/pedropenha/transcreveai) (público). Remote `upstream` = Handy, com push bloqueado. Correções do Handy entram por cherry-pick seletivo (ADR-0001).
- Commits convencionais (`feat:`, `fix:`, `docs:`, `refactor:`, `chore:`), com a mensagem focada no porquê.

### Releases (git flow)

- `main` é a branch de release; `integration/v1` acumula o trabalho da v1.
- Cada release sai de uma branch `release/x.y.z` tirada da branch de
  integração: nela entram só o bump de versão (`package.json`,
  `src-tauri/Cargo.toml`, `src-tauri/tauri.conf.json` — os três são
  independentes, não há sincronia automática) e correções de estabilização.
  Depois ela é mesclada em `main` e de volta na integração.
- A tag `vX.Y.Z` **não é criada à mão**: o `release.yml` (manual, via
  `workflow_dispatch`) lê a versão do `tauri.conf.json`, e o `tauri-action`
  cria a tag no ref disparado e sobe os artefatos. Dispare sempre a partir
  de `main`, para a tag não nascer apontando para uma branch de trabalho.
- A versão **tem de subir** a cada release: o updater só oferece versão
  maior que a instalada.
- As 66 tags `v0.1.0`…`v0.9.7` herdadas do Handy ficaram só no clone local,
  fora do `origin` — elas apontam para o histórico do Handy e colidiriam com
  as nossas releases.
