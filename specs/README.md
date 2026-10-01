# Transcreve.ai — Especificações (SDD)

> **Transcreve.ai** é um app de ditado por voz + notas de reunião que replica o comportamento do **Wispr Flow**, com transcrição **local** (whisper.cpp / Parakeet) ou via **API paga com chave própria** (OpenAI, Groq, Deepgram ou endpoint compatível com OpenAI). **Na v1 só existe STT local**; STT em nuvem chega na v1.1+ ([ADR-0002](../docs/adr/0002-escopo-v1.md)).

## Hierarquia

| Assunto                                                                                                            | Quem manda                                                      |
| ------------------------------------------------------------------------------------------------------------------ | --------------------------------------------------------------- |
| Engenharia: processo, TDD, cobertura, testes, revisão, estilo, erros, segurança de código, git, design de frontend | **ECC rules** (`~/.claude/rules/ecc/`) e **ECC skills/agentes** |
| Produto: comportamento, escopo, prioridades                                                                        | **Estas specs** (referência: Wispr Flow)                        |

Detalhes em [constitution.md](constitution.md#0-hierarquia-de-autoridade). Se uma spec contradizer uma rule, a rule vence e a spec é corrigida.

## Mapeamento com o workflow do ECC

`rules/common/development-workflow.md` pede documentos de planejamento antes de codar. Eles já existem aqui:

| ECC pede                     | Documento                                                                                                                                                     |
| ---------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| PRD                          | [product/product-spec.md](product/product-spec.md) + [features/](features/)                                                                                   |
| Architecture / system design | [architecture/plan.md](architecture/plan.md)                                                                                                                  |
| Tech doc                     | [architecture/contracts.md](architecture/contracts.md) + [architecture/data-model.md](architecture/data-model.md)                                             |
| Task list                    | [tasks.md](tasks.md)                                                                                                                                          |
| Research & reuse (passo 0)   | [product/research-wispr-flow.md](product/research-wispr-flow.md) + tarefa **T-000** → [ADR-0001](../docs/adr/0001-fork-do-handy-como-base.md) (fork do Handy) |
| Decisões de arquitetura      | [docs/adr/](../docs/adr/README.md)                                                                                                                            |

## Índice

| Documento                                                                        | Conteúdo                                                      | Release                           |
| -------------------------------------------------------------------------------- | ------------------------------------------------------------- | --------------------------------- |
| [constitution.md](constitution.md)                                               | Hierarquia e princípios de produto                            | —                                 |
| [product/research-wispr-flow.md](product/research-wispr-flow.md)                 | Como o Wispr Flow funciona (pesquisa + prints)                | —                                 |
| [product/product-spec.md](product/product-spec.md)                               | Visão, personas, escopo, jornadas, glossário                  | —                                 |
| [architecture/plan.md](architecture/plan.md)                                     | Stack, módulos, janelas, fluxos, riscos, mapa de rules/skills | —                                 |
| [architecture/data-model.md](architecture/data-model.md)                         | SQLite, arquivos, retenção                                    | —                                 |
| [architecture/contracts.md](architecture/contracts.md)                           | Traits de provedores, APIs externas, IPC                      | —                                 |
| [features/001-flow-bar](features/001-flow-bar/spec.md)                           | Barra flutuante (idle, hover com 2 opções, estados)           | v1                                |
| [features/002-hotkeys-dictation](features/002-hotkeys-dictation/spec.md)         | Atalhos globais e sessão de ditado                            | v1                                |
| [features/003-transcription-engines](features/003-transcription-engines/spec.md) | Motores locais e em nuvem, modelos, chaves                    | v1 (local) · v1.1+ (nuvem)        |
| [features/004-text-pipeline](features/004-text-pipeline/spec.md)                 | Limpeza, dicionário, snippets, estilos                        | v1 (determinístico) · v1.1+ (LLM) |
| [features/005-text-insertion](features/005-text-insertion/spec.md)               | Inserir texto no app com foco                                 | v1                                |
| [features/006-command-mode](features/006-command-mode/spec.md)                   | Editar texto selecionado por voz                              | v1.1+                             |
| [features/007-notes-scratchpad](features/007-notes-scratchpad/spec.md)           | Notas por voz (Scratchpad)                                    | v1.1+                             |
| [features/008-meeting-detection](features/008-meeting-detection/spec.md)         | Detectar reunião e mostrar popup                              | v1                                |
| [features/009-meeting-notetaker](features/009-meeting-notetaker/spec.md)         | Gravar, transcrever e resumir reuniões                        | v1                                |
| [features/010-hub-settings](features/010-hub-settings/spec.md)                   | Hub, histórico, configurações, onboarding, bandeja            | v1                                |
| [features/011-security-privacy](features/011-security-privacy/spec.md)           | Segredos, dados, rede, consentimento                          | v1                                |
| [features/012-voice-assistant](features/012-voice-assistant/spec.md)             | Assistente por voz no overlay + providers de agentes CLI      | v1.1+ (antecipável)               |
| [tasks.md](tasks.md)                                                             | Plano de implementação por fases                              | —                                 |

## Convenções

- **IDs**: `FR-<feature>-<nn>` (funcional), `NFR-<feature>-<nn>` (não funcional), `AC-<feature>-<nn>` (aceitação), `US-<feature>-<nn>` (história), `T-<nnn>` (tarefa).
- **Prioridade**: `P0` = obrigatório no release · `P1` = desejável · `P2` = futuro.
- **`[NEEDS CLARIFICATION]`** marca decisões em aberto, sempre com uma proposta padrão.
- Critérios de aceitação em **Dado / Quando / Então** — viram testes conforme `rules/common/testing.md`.

## Como usar com o Claude Code

- "Implemente a `T-012` seguindo `specs/tasks.md`" — o fluxo (planner → TDD → reviewers → verification-loop → commit) vem das ECC rules; ver [tasks.md](tasks.md#fluxo-de-cada-tarefa).
- "Revise se `src-tauri/src/insertion` satisfaz os `AC-005-*`."
- Ao concluir, marque a tarefa em `tasks.md` e registre desvios de produto na spec da feature.
