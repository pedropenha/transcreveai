# ADR-0002: Escopo do release v1 (ditado + notetaker de reuniões)

**Date**: 2026-09-30
**Status**: accepted
**Deciders**: usuário (produto), agente executor (loop de implementação)

## Context

O plano original (`specs/tasks.md`, fases 1–4) previa quatro releases incrementais: v0.1 (ditado), v0.2 (texto inteligente), v0.3 (reuniões), v1.0 (polimento). A estratégia mudou para um único release v1 amplo, executado por um loop de agentes sobre a branch `integration/v1`, com merges seriais por task. Isso exige fixar o que entra e o que fica de fora para que as lanes paralelas não reabram decisões de produto.

## Decision

O **v1** entrega: ditado completo (PTT, mãos livres, Flow Bar, inserção), notetaker de reuniões (detecção, gravação mic+sistema, transcrição, resumo) e todas as telas do Hub, no visual da T-008. Sem provedores de STT em nuvem. Decisões de produto fixas:

| Tema                    | Decisão                                                                                                                                                                                    |
| ----------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| Escopo v1               | Ditado + notetaker de reuniões + todas as telas; estilo da T-008                                                                                                                           |
| Locales                 | Só pt-BR + en (demais removidos na T-005)                                                                                                                                                  |
| STT em nuvem            | Nenhum na v1 (T-014 adiada para v1.1+)                                                                                                                                                     |
| Modelo first-run        | Usuário escolhe no onboarding; `large-v3-turbo` (Turbo) é o default recomendado                                                                                                            |
| Inserção                | `auto` é o padrão; janela elevada (UIPI) → `clipboard_only` + aviso                                                                                                                        |
| Retenção                | "Apagar todos os dados" inclui o áudio preservado de sessões falhas (T-022)                                                                                                                |
| Fila de sessões         | N = 5 pendentes; limite de 5 min por gravação; configurável no Avançado                                                                                                                    |
| Limpeza                 | `light` determinística: muletas pt-BR ("né", "tipo", "aí", "ahn/ééé", "então assim", repetições) + pontuação/capitalização; lista editável na tela Dicionário (T-044). LLM fica para v1.1+ |
| Flow Bar                | Posição inferior-centro; sons ligados por padrão; soneca 15/30/60 min                                                                                                                      |
| Resumo de reunião       | LLM BYOK (T-050 + T-016 keyring). Sem chave: transcrição e notas funcionam, resumo desabilitado                                                                                            |
| Apps detectados (T-061) | Zoom, Teams, Meet (navegador), Webex                                                                                                                                                       |
| Verificação             | Testes mockados/automatizados; smokes reais viram checklist manual de entrega                                                                                                              |
| Release                 | `bun run tauri build` local (NSIS sem assinatura/updater — T-049 não se aplica)                                                                                                            |

Tasks do v1 (~33): T-002–T-007, T-009 (+fixtures WAV pt-BR), T-010–T-013, T-015, T-016, T-020–T-022, T-030, T-031, T-035, T-040–T-046, T-050, T-060–T-069. Adiadas para v1.1+: T-014, T-017, T-032, T-049, T-051–T-057, T-080–T-085. T-043 vira "só modelos locais"; T-069 vira roteiro manual parcial (só os pontos automatizáveis viram teste).

## Alternatives Considered

### Alternativa 1: manter os releases fatiados (v0.1 → v0.3 → v1.0)

- **Pros**: valor incremental; cada fase já era testável isoladamente.
- **Cons**: três empacotamentos e três rodadas de smoke manual; o notetaker é o diferencial do produto e ficaria para o fim; overhead de release não se paga sem canal de distribuição.
- **Why not**: sem updater nem distribuição na v1, releases intermediários são só tags — o loop entrega mais rápido mirando direto o v1.

### Alternativa 2: incluir STT em nuvem (T-014) no v1

- **Pros**: cobre máquinas fracas; já existe contrato `SttProvider` pensado para isso.
- **Cons**: exige T-016 antes de qualquer UI de chave, mais uma superfície de segurança e testes de rede; o hardware-alvo do dev (GPU) já roda whisper local bem.
- **Why not**: LLM BYOK (T-016+T-050) já entra por causa do resumo de reunião — a mesma infraestrutura de segredos torna a T-014 barata na v1.1+, sem bloquear o v1.

## Consequences

### Positive

- Um único release testável de ponta a ponta, com o diferencial (reuniões) incluído.
- Sem STT em nuvem: menos superfície de segurança/rede no v1; modo offline trivial.
- Merge seral em `integration/v1` com `LOOP-STATE.md` como memória entre sessões.

### Negative

- Escopo grande num único release: regressões só aparecem tarde no smoke manual.
- Sem resumo de reunião para quem não configurar chave LLM (degradação explícita, documentada).

### Risks

- Loopback WASAPI e detecção de reunião são partes novas sem equivalente no Handy — mitigação: testes mockados com timelines simuladas + checklist manual no fim.
- Tasks v1.1+ adiadas ainda aparecem nas specs de feature — mitigação: cada spec marca o release por requisito (`v1` vs `v1.1+`).
