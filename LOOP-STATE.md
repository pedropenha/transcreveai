# LOOP-STATE — Transcreve.ai v1

> Memória do loop entre sessões. Atualizar ao fim de cada task: task atual, lanes, merges pendentes, bloqueios.
> Governança: [ADR-0002](docs/adr/0002-escopo-v1.md) · plano: [specs/tasks.md](specs/tasks.md)

## Estado

- **Alvo de merges**: `integration/v1` (worktree `C:\multimidia\ecc-v1`). `main` nunca é tocada.
- **Base**: `chore/t-001a-baseline` + `chore/t-008-design-direction` (já mergeada em `3a82c14`).
- **Última atualização**: 2026-09-30 — Passo 0 concluído.

## Concluído

- [x] Passo 0 — Governança: `integration/v1` criada em `ecc-v1`; ADR-0002; `specs/tasks.md` emendado (T-001/T-001a/T-008 `[x]`; T-014, T-017, T-032, T-049, T-051–T-057, T-080–T-085 → v1.1+; T-043 só modelos; T-069 manual parcial); specs corrigidas (README, product-spec, F001–F005, F008–F011, plan.md); `bindings.ts` commitado em `chore/t-001a-baseline` (`2179f07`).
- [x] T-001, T-001a, T-008 (pré-loop)

## Em andamento

- (nenhuma)

## Próximas na DAG (prontas para lanes)

- **T-002** [P] CI (depende de T-001a ✔)
- **T-003** [P] logging — sem deps de código
- **T-004** [P] SQLite/schema — sem deps de código
- **T-005** [P] settings + i18n pt-BR/en
- **T-006** [P] IPC/specta
- **T-007** instância única/bandeja
- **T-009** [P] base E2E + fixtures WAV pt-BR
- **T-010** engine de áudio · **T-020** hook de teclado · **T-015** [P] modelos · **T-016** [P] keyring · **T-045** onboarding (dep. T-043; pode ir depois)

## Merges pendentes

- (nenhum)

## Bloqueios

- (nenhum)

## Notas operacionais

- Conflitos esperados entre lanes: `src/bindings.ts` (regenerar após cada merge), `src/i18n/locales/en/translation.json`, `specs/tasks.md`.
- Windows: cargo/`bun run tauri` só em PowerShell com `. .\scripts\windows-dev-env.ps1 -BypassJunction`.
- Catracas da T-001a: clippy `-D warnings`, `cargo deny`/`audit`, ≤12 arquivos >800 linhas, ≤146 unwrap/expect, ≤50 unsafe. Nunca regredir.
