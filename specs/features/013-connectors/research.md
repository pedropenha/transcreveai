# F013 — Pesquisa e decisão de reuso

**Consulta**: 2026-10-05 · fontes primárias · ECC `search-first`, `api-connector-builder`, `mcp-server-patterns`.

## Direção revisada após pesquisa

Usuário escolheu OAuth por conta em 2026-10-05. Recomendação atual: núcleo de política independente da IA, Notion como cliente MCP oficial OAuth e Azure REST com Entra. [OAuth e persistência](oauth.md) registra fontes e gate de callback. A comparação de PAT abaixo conserva alternativas pesquisadas, não a jornada escolhida.

## Evidência local

Checkout `C:\multimidia\ecc`, branch `integration/v1`, limpo antes deste planejamento. Inspecionados `llm/provider.rs`, `llm/types.rs`, `llm/router.rs`, `llm/openai_compat.rs`, `llm/anthropic.rs`, `llm/http.rs`, `llm/cli_agent/{adapters,spawn,validate}.rs`, `secrets.rs`, F011/F012 e contratos.

Há dois adapters BYOK e CLI atrás de `LlmProvider::complete`, contrato somente texto. Codex usa `--ignore-user-config`, `--ignore-rules`, read-only, ephemeral e JSON; parâmetros de configuração/MCP são rejeitados pelo validador. `spawn` limpa ambiente. O cofre e a política offline existentes são os pontos de reuso; não há conector Notion/Azure nem host MCP no núcleo inspecionado. O transporte LLM atual não cobre genericamente PATCH/JSON Patch; adaptar infraestrutura HTTP sem acoplar erros de conector a `LlmError`.

## Comparação

| Opção                            | Autenticação/capacidade                             | Manutenção/custo operacional                                  | Decisão proposta                                                                        |
| -------------------------------- | --------------------------------------------------- | ------------------------------------------------------------- | --------------------------------------------------------------------------------------- |
| Notion MCP hospedado oficial     | OAuth; ferramentas próprias de pesquisa/conteúdo    | Serviço mantido pelo Notion, conexão OAuth necessária         | Boa opção para uso direto em cliente MCP; P2 no app com política comum                  |
| Notion MCP local oficial OSS     | Token de API, ferramentas derivadas da API          | MIT; repositório informa ausência de manutenção/suporte ativo | Não embutir como dependência principal nova                                             |
| Notion REST                      | PAT pessoal ou conexão interna/OAuth                | Contratos tipados e subset mantidos por nós                   | P0 para autenticação por token e escopo do produto                                      |
| Azure DevOps MCP local Microsoft | PAT ou outros modos documentados; catálogo amplo    | MIT, Node 20+, pacote oficial `@azure-devops/mcp`             | Reutilizável por cliente externo; alternativa se spike comprovar controle/empacotamento |
| Azure DevOps MCP remoto          | Microsoft Entra; compatibilidade depende do cliente | Hospedado pelo Azure DevOps                                   | Não assumir PAT nem compatibilidade Codex atual                                         |
| Azure REST                       | PAT, permissões do usuário e scopes                 | Subset de Boards com HTTP existente do app                    | P0 para PAT e operações delimitadas                                                     |
| MCP próprio stdio sobre o núcleo | Pareamento local; PAT fica no app                   | SDK oficial, sem runtime Node obrigatório                     | P1 para interoperabilidade com mesma política                                           |

Conclusão original para PAT era API direta + fachada MCP. Após escolha de OAuth, reutilizar MCP hospedado Notion e REST Entra Azure dentro da mesma política; isso é inferência arquitetural a validar no spike. MCPs oficiais são preferíveis para exploração direta em clientes externos quando sua autenticação e catálogo bastam. Não reconstruir SDK/protocolo MCP, OAuth ou cliente HTTP.

## Descobertas verificadas

Notion tem PAT user-scoped e token de conexão interna. O PAT usa as permissões do usuário e depende da política do workspace; o token interno usa páginas compartilhadas com a conexão. Notion orienta OAuth público para produto usado por muitos usuários. [PAT](https://developers.notion.com/guides/get-started/personal-access-tokens), [autorização](https://developers.notion.com/guides/get-started/authorization).

O MCP hospedado Notion usa OAuth. O repositório MCP local recomenda o remoto e declara não estar sendo mantido/suportado ativamente; não confundir a existência do pacote com recomendação de adotá-lo. [MCP oficial](https://developers.notion.com/guides/mcp/overview), [repositório e licença](https://github.com/makenotion/notion-mcp-server).

Notion exige versionamento explícito; planejar `Notion-Version: 2026-03-11` e distinguir database/data source nos contratos. Leituras e escrita seguem limites documentados, incluindo 429/Retry-After. Não presumir que busca REST equivale à busca do MCP hospedado. [Versionamento](https://developers.notion.com/reference/versioning), [limites](https://developers.notion.com/reference/request-limits).

Azure oferece MCP remoto e local. Remoto exige organização respaldada por Entra e autenticação compatível do cliente; docs orientam local para clientes como Codex nos casos sem fluxo compatível. PAT pedido pelo usuário encaixa no local ou REST. [Microsoft Learn](https://learn.microsoft.com/en-us/azure/devops/mcp-server/mcp-server-overview?view=azure-devops).

No MCP local Microsoft, `--authentication pat` lê `PERSONAL_ACCESS_TOKEN` com base64 de `email:PAT`; isso é específico do adapter, não um token MCP genérico. Não copiar segredo para arquivo ou comando conforme exemplos simplificados. A API REST pode usar Basic de usuário:PAT; validar encoding em testes sem logar valor. [Getting Started](https://github.com/microsoft/azure-devops-mcp/blob/main/docs/GETTINGSTARTED.md), [implementação de auth](https://github.com/microsoft/azure-devops-mcp/blob/main/src/auth.ts), [repositório/licença](https://github.com/microsoft/azure-devops-mcp).

Azure Work Items Update usa JSON Patch e suporta teste da revisão. Planejar API 7.1 para Boards; confirmar scopes e campos por endpoint/processo no spike. [Contrato update](https://learn.microsoft.com/en-us/rest/api/azure/devops/wit/work-items/update?view=azure-devops-rest-7.1).

Codex documenta stdio e Streamable HTTP, autenticação e allowlist de ferramentas. A configuração pessoal descrita na documentação não é herdada pelo provider isolado deste app. Registrar compatibilidade por versão executada, principalmente flags de isolamento/config e comportamento headless. [Codex MCP](https://developers.openai.com/codex/mcp/).

MCP define transporte stdio e HTTP; uma API cloud não inicia um processo stdio no desktop do usuário. Usar host local para traduzir tool calls ou contexto/propostas. Para Rust há SDK oficial, evitando segundo runtime de produção. [Transportes](https://modelcontextprotocol.io/specification/2025-06-18/basic/transports), [Rust SDK](https://github.com/modelcontextprotocol/rust-sdk).

## Limites desta pesquisa

Pesquisa pública de documentação, código/repositórios e identificação de pacotes; não houve instalação, consulta autenticada de workspace/projeto, benchmark ou teste de interoperabilidade. Nenhuma versão de pacote foi fixada sem spike; branch `main` não prova versão publicada. Licenças e dependências transitivas devem ser conferidas na versão adotada.

Não foi feito inventário exaustivo de npm/crates.io, nem utilizado GitHub code search autenticado/Context7/Exa. Pesquisa usa repositórios oficiais públicos e docs primárias; não concluir ausência de alternativas comunitárias. O SDD deixa essas verificações e aceitação funcional na T-094/T-100.
