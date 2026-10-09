# F013 — Implementação por lanes ECC

Data: 2026-10-05. Implementação autorizada pelo usuário após SDD/OAuth e jornadas de duas fases. Worktree: `C:\multimidia\ecc`, branch `integration/v1`. Alterações de documentação do planejamento já existiam; preservadas. Sem autorização para commit/push nesta etapa.

## Lanes e ordem

| Lane / papel ECC                      | Modelo           | Superfície de escrita                                 | Gate / verificação                                                   |
| ------------------------------------- | ---------------- | ----------------------------------------------------- | -------------------------------------------------------------------- |
| Backend / tdd-guide Rust              | gpt-6.1-sol      | connectors e commands/connectors novos                | Testes de domínio, OAuth/refresh, segredo, offline, catálogos        |
| Interface / tdd-guide React           | gpt-6-sol        | settings/connectors, navegação de settings e en/pt-BR | Testes de estado/defaults e interação                                |
| Spike / architect + security-reviewer | gpt-6-astra      | leitura/revisão                                       | SDK/metadata, callbacks, scopes/registro Entra, achados de segurança |
| Integração / orquestrador             | modelo da sessão | wiring, dependências, bindings, bridge e docs         | Build/test, smoke, revisões, evidência por fase                      |

Contratos são compartilhados entre lanes antes de integrar; ninguém modifica a superfície de outra lane sem handoff. Cargo/build centralizado para evitar concorrência no mesmo target. Modelo diferente não substitui revisão independente.

Plano já aprovado por "pode implementar". Aplicam-se `orch-add-feature`, `parallel-execution-optimizer`, TDD e revisão ECC. Gate fase 1 autenticação/configuração/MCP de leitura precede publicação de fase 2. Requisitos de produto explicitamente autorizados passam a ser exceção de integração na constituição; release v1 não é ampliado automaticamente.

## Dependências externas

- Azure requer application/client ID de registro Entra e permissões/redirect de cliente público compatíveis. Não inventar ID nem usar app de terceiros. Interface informa o registro necessário.
- Notion OAuth MCP requer registration/discovery e callback suportados. Nenhum segredo global de aplicação no desktop.
- Aceitação autenticada usa conta/destinos de teste; não confundir suporte implementado com login/permissão validada sem credenciais.

Resultados de cada lane, checks e limites serão registrados após execução; este documento não declara gates completos.
