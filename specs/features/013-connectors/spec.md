# F013 — Conectores Notion e Azure DevOps, independentes do provider de IA

**Status**: Draft / proposta para implementação · **Data**: 2026-10-05 · **Release**: v1.1+ (proposta) · **Depende de**: T-016, T-050, F011, F012

## Objetivo e contexto

Consultar informações e transformar ditados, notas e reuniões em páginas do Notion e work items do Azure DevOps. A mesma conexão deve funcionar com Codex CLI autenticado ou LLM por chave própria (BYOK). Configurar um conector não deve exigir escolher uma IA; exportação manual não exige IA.

Pedido do usuário: especificar com SDD, pesquisar reuso via ECC, incluir autenticação por PAT e avaliar MCP próprio/externo. Este documento autoriza planejamento; não afirma implementação ou conectividade já entregues.

Direção atual: OAuth por conta, núcleo Rust com políticas no app, Notion via MCP oficial hospedado e Azure DevOps via REST com Microsoft Entra. Fachada MCP local da fase 1 reutiliza essas conexões. Detalhes e gates: [OAuth e persistência](oauth.md). MCP é protocolo de ferramentas; a LLM precisa de um host que execute essas ferramentas. Uma chave de LLM, um PAT de serviço e autenticação MCP têm finalidades diferentes.

Pesquisa: [research.md](research.md). Arquitetura e contratos: [plan.md](plan.md). Decisão: [ADR-0004](../../../docs/adr/0004-conectores-e-mcp-independentes-do-provider.md). Jornadas e duas fases: [notetaker.md](notetaker.md). Tarefas: T-094..T-103 em [tasks.md](../../tasks.md).

## Escopo proposto

Escopo confirmado em duas fases: **1 — disponibilizar/instalar e configurar MCPs**, OAuth, diagnóstico, leitura e defaults de projeto/equipe/backlog/sprint; **2 — ações no resumo do Notetaker**, vincular/publicar no Notion e criar Task/Feature/item de backlog sugerido pela IA com revisão de destino/sprint. Configurações separadas de Modelos & Provedores, com pt-BR/en. Detalhes e gates em [notetaker.md](notetaker.md).

Fachada MCP própria stdio para clientes locais (incluindo Codex), baseada no mesmo núcleo, integra a fase 1 para leitura e configuração; propostas/publicação são habilitadas apenas na fase 2. Distribuição do bridge acompanha o instalador; aprovação permanece no app.

P2: MCP remoto arbitrário, OAuth REST público do Notion com broker, Streamable HTTP próprio, tool calling iterativo dentro do app, anexos, sincronização bidirecional/webhooks, Azure DevOps Server/on-premises, PRs/pipelines/repos/test plans, exclusão ou mudança de permissões. Não fazem parte do primeiro incremento.

Não existe conexão de saída silenciosa, instalação silenciosa de MCP de terceiros ou herança da configuração pessoal de MCP do Codex.

## Histórias

- **US-013-01** Quero conectar meu Azure DevOps entrando na conta Microsoft e escolher organização, projeto e tipos de work item.
- **US-013-02** Quero conectar meu Notion e escolher onde consultar/publicar notas e tarefas.
- **US-013-03** Quero consultar tarefas e documentos com Codex ou BYOK, mantendo minhas conexões.
- **US-013-04** Quero revisar título, conteúdo e destino antes de publicar uma reunião ou tarefa.
- **US-013-05** Quero adicionar o MCP do Transcreve.ai a um cliente local sem entregar PATs à IA.
- **US-013-06** Quero exportar manualmente mesmo sem IA configurada.
- **US-013-07** Quero pedir ao assistente que consulte backlog, work items ou páginas das minhas conexões e cite a origem — sabendo que ele nunca cria nem altera nada; criação só acontece na revisão da reunião.

## Requisitos funcionais

### Conexão e autenticação

- **FR-013-01** Conectores têm identidade própria, distinta do provider LLM. Permitir múltiplas conexões com nomes e IDs estáveis; cada sessão seleciona conexões explicitamente.
- **FR-013-02** Notion: conectar por OAuth no MCP oficial hospedado, com cliente registrado, PKCE e escolha de escopo no app. Não pedir PAT/token interno na jornada principal. Registro/redirect desktop são gate do spike; ver oauth.md.
- **FR-013-03** Azure: conectar via OAuth Microsoft Entra, aplicativo desktop public client sem client_secret global, PKCE e autorização delegada para Azure DevOps. Selecionar tenant/organização/projetos e respeitar consentimento corporativo. Não usar OAuth legado Azure DevOps nem prometer compatibilidade do MCP remoto com Codex.
- **FR-013-04** Persistir access/refresh tokens e eventual segredo de registro por instalação somente no cofre do SO, com namespace de conexão. Frontend recebe estado e identidade, nunca tokens. Refresh serializado, bundle atômico, rotação e invalid_grant conforme oauth.md. Desconectar/revogação invalida grants e aprovações; nenhuma credencial em config MCP, prompts, argv, banco ou logs.
- **FR-013-05** Testar conexão com leitura mínima: validar identidade e acesso ao destino escolhido. Distinguir credencial inválida, acesso insuficiente, recurso inacessível, política administrativa, modo offline, timeout e limite de requisições. Teste de conexão não cria conteúdo nem prova permissão de escrita. Capacidade de publicação fica `unknown` quando não houver evidência confiável; mostrar "permissão de publicação ainda não verificada". Não presumir introspecção do scope do PAT por leitura; PermissionDenied em envio aprovado é falha recuperável, sem ampliar privilégios.
- **FR-013-06** Allowlist por conexão: Notion por páginas-raiz/data sources; Azure por organização/projetos. Validar no backend em cada operação, inclusive lookup por ID e continuação paginada. Não enviar resultados fora do escopo ao frontend/modelo.

### Leitura e publicação

- **FR-013-07** Notion: mapear ferramentas do MCP oficial para leitura delimitada de páginas e consulta/criação em destinos suportados. Descobrir schemas e fixar mapping allowlisted validado no spike. Ferramenta ausente retorna UnsupportedCapability, sem fallback PAT/REST implícito; não expor catálogo inteiro ao modelo.
- **FR-013-08** Azure: consultar work items por filtros estruturados/projeto, obter detalhes, criar e atualizar campos permitidos via REST com token Entra. Consultar tipos/campos/estados por processo; não fixar Task/User Story/Bug/estados universalmente. Modelo não fornece WIQL/JSON Patch/URL livre.
- **FR-013-09** Exportar notas/resumo selecionados manualmente, preservando o original local. Mostrar destino, título, corpo, propriedades/campos, atribuição e dados enviados. Resolver mapeamentos obrigatórios antes da confirmação; formato não suportado exige edição, não descarte silencioso.
- **FR-013-10** Assistente/sugestões na fase 2: usuário escolhe conexão e leitura/filtro; backend busca contexto limitado; `LlmProvider` atual recebe dados e retorna texto ou proposta estruturada validada. O modelo nunca executa a publicação. JSON inválido vira rascunho/erro recuperável; não inferir comandos a partir de prosa.
- **FR-013-11** Toda escrita exige revisão de proposta concreta e confirmação no app. Autorização vincula usuário local, conexão, operação, destino, payload, versão/política e prazo; qualquer mudança invalida a aprovação. Consulta e geração de rascunho não exigem confirmar cada chamada após opt-in da conexão.
- **FR-013-12** Ciclo de operação: draft → awaiting_approval → executing → succeeded | failed | outcome_unknown; denied/cancelled antes do envio. Confirmar duas vezes não duplica. Timeout após envio pode ter efeito remoto: investigar/reconciliar antes de permitir novo envio; nunca repetir criação cegamente. Não declarar rollback ao cancelar chamada já enviada.
- **FR-013-13** Atualização Azure usa revisão esperada e operação de teste de `/rev`; conflito exige reler/revisar. Notion permite criação de subpágina ou append seguro ao resumo na fase 2, sem substituição destrutiva da página; criação/append podem duplicar após resultado desconhecido e exigem reconciliação.
- **FR-013-14** Sucesso mostra ID/link remoto verificável e estado da operação. Publicação parcial de lote mostra resultado por item; primeira versão publica uma proposta por vez e não promete transação entre serviços.

### MCP e providers

- **FR-013-15** MCP próprio local usa stdio e SDK oficial, expondo somente capacidades allowlisted de leitura e criação de propostas. Não expor HTTP genérico, execução de shell, leitura de segredo ou ferramenta de autoaprovação.
- **FR-013-16** Bridge MCP consulta o app ativo por IPC local protegido; não abre listener TCP nem inicia captura. Cliente é pareado explicitamente e recebe acesso delimitado a conexões/operações. Persistir dono de cada proposta/operação (UI ou client_id/grant_id); consultas MCP exigem ownership e grant ativo mesmo com ID conhecido. Cliente não escolhe seu owner nem consulta operações de outro cliente. App ausente, cliente não pareado, conexão removida ou modo offline falham de modo fechado.
- **FR-013-17** Codex usado hoje para texto/resumo continua isolado, sem MCP pessoal. No app, fase 2 usa contexto e propostas via `LlmProvider`; MCP da fase 1 pode ser adicionado a uma sessão externa do Codex pelo usuário. Habilitar MCP no subprocesso interno é incremento separado, condicionado a validação de versão/configuração controlada, nunca removendo os flags de isolamento globalmente.
- **FR-013-18** BYOK na fase 2 também usa contexto/propostas; funciona mesmo sem suporte nativo a MCP/tool calling na API. Tool calling iterativo futuro exige adaptador de capacidade e host local que traduza tool calls e aplique a mesma política. LLM cloud não alcança stdio/localhost do usuário diretamente.
- **FR-013-19** Gerar exemplo de configuração MCP sem segredo, com caminho absoluto do bridge. Copiar/exportar é ação do usuário; não alterar `~/.codex/config.toml` automaticamente. Não prometer compatibilidade com qualquer cliente sem teste.
- **FR-013-20** Provedor LLM recebe apenas contexto escolhido/limitado, nunca tokens dos serviços. Conteúdo remoto é dado não confiável, não instrução; resultados citam origem/link e truncamento. Recusar destinos ou ferramentas sugeridos por conteúdo recuperado fora da política.
- **FR-013-32** Assistente pode consultar conectores somente por comando explícito do usuário (seleção de conexão/consulta na UI ou comando textual equivalente). O backend executa a leitura allowlisted e injeta o resultado como contexto delimitado e rotulado como dado não confiável, com origem/link. Funciona por injeção de contexto em Codex e BYOK sem exigir tool calling; não inferir consulta a partir de prosa ambígua nem acionar busca automaticamente a cada mensagem.
- **FR-013-33** O assistente não possui capacidade de escrita: nenhuma operação de criação/alteração de conector é exposta ao modelo, ao contexto ou a ferramentas do assistente. A exigência não é instrução de prompt — a capacidade não existe nesse caminho e a policy do backend nega mutações fora do ciclo de aprovação. Criação/edição ocorre somente por proposta revisada e confirmada na UI da reunião (FR-013-11).

### UX e privacidade

- **FR-013-21** Configurações → Conectores: Conectar pelo navegador, selecionar conta/escopo, testar, Reconectar e Desconectar. Estados: não conectado, autorizando, pronto, leitura apenas, permissões desconhecidas, reconexão necessária, offline e falha temporária. Token salvo/autenticação válida não prova acesso ao destino ou escrita.
- **FR-013-22** Privacidade mostra dois trajetos: dados enviados ao serviço para consulta/publicação; contexto enviado ao provider LLM escolhido. Seleção de Codex continua representando envio ao provider do CLI. Sem fallback de provider nem sincronização em background implícita.
- **FR-013-23** Offline bloqueia leitura/publicação/teste, via UI ou MCP, antes da rede. Preserva texto/propostas locais; não enfileira replay automático ao voltar online. Excluir conexão apaga segredo e revoga grants; apagar todos os dados inclui conexões, grants e journal.

Requisitos **FR-013-24..31** e critérios **AC-013-21..28** em [notetaker.md](notetaker.md) completam esta especificação e são obrigatórios nas respectivas fases.

## Requisitos não funcionais

- **NFR-013-01** Engenharia segue ECC: TDD, cobertura mínima 80% do escopo alterado, revisão de segurança/idiomática, tipos gerados, i18n en/pt-BR e testes de comportamento. Não tratar testes com mocks como aceite nativo autenticado.
- **NFR-013-02** Defaults propostos: timeout por chamada 30 s; 50 resultados por página; até 3 páginas e 32 KiB de contexto por turno. Indicar truncamento e continuação explícita; impor limite de bytes antes de parsing. Cancelamento e deadline propagados.
- **NFR-013-03** Rate limit por conexão; respeitar Retry-After, backoff com jitter, até duas novas tentativas somente para leituras seguras e dentro do deadline. Escrita sem idempotência garantida não repete após falha ambígua. Cursor tratado como opaco e validado com vínculo à conexão/filtro.
- **NFR-013-04** TLS, host allowlist, redirects controlados sem transportar Authorization a outra origem; proxy conforme F011. Não aceitar URLs arbitrárias do modelo. Publicar HTML/Markdown com conversão segura e validar links; conteúdo remoto não vira HTML executável no Hub.
- **NFR-013-05** Auditoria local mínima: ID da operação/conexão, ação, data, estado e referência remota; sem corpo/prompt/segredo em logs. Rascunho explicitamente salvo segue retenção de notas; payload em execução tem retenção curta definida no plano. Sem telemetria.

## Aceitação e rastreabilidade

| ID        | Dado / Quando / Então                                                                                                                                                                                                                                           | Requisitos        | Tarefas       |
| --------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ----------------- | ------------- |
| AC-013-01 | Dado OAuth Notion autorizado, quando testo identidade e destino via MCP, então leitura funciona e nenhum conteúdo é criado; negar consentimento/revogar acesso gera orientação correta.                                                                         | FR-02,04,05       | T-094,095     |
| AC-013-02 | Dado OAuth Entra e acesso de leitura Azure, quando consulto projeto permitido, então recebo itens; escrita informa capacidade conhecida ou unknown; envio aprovado negado gera PermissionDenied sem ampliar acesso.                                             | FR-03,05,08       | T-094,096     |
| AC-013-03 | Dado dois providers, quando alterno Codex/BYOK e consulto o mesmo escopo, então conexões continuam funcionando e credenciais não entram nas chamadas LLM.                                                                                                       | FR-01,10,17,18,20 | T-097         |
| AC-013-04 | Dada proposta válida, quando aprovo, então um recurso é publicado; negar ou alterar payload/destino após aprovação impede execução.                                                                                                                             | FR-09,11,12,14    | T-097,098     |
| AC-013-05 | Dado resultado fora do escopo ou ID direto não permitido, quando UI/MCP consulta, então backend nega sem entregar conteúdo ao modelo.                                                                                                                           | FR-06,15,16       | T-094,099     |
| AC-013-06 | Dado token OAuth canário, quando examino logs, banco, settings, argv, prompts e export MCP, então não encontro token nem sua forma Basic/base64.                                                                                                                | FR-04,19,20       | T-094,100     |
| AC-013-07 | Dado offline, quando UI/MCP tenta teste, leitura ou publicação, então nenhuma requisição externa ocorre e rascunho fica acessível.                                                                                                                              | FR-16,23          | T-094,099,100 |
| AC-013-08 | Dado timeout após envio ou duplo clique, quando usuário tenta novamente, então não ocorre repetição cega e estado desconhecido exige reconciliação.                                                                                                             | FR-12             | T-097,100     |
| AC-013-09 | Dado work item alterado remotamente, quando publico atualização com revisão antiga, então conflito impede sobrescrita e exige nova revisão.                                                                                                                     | FR-13             | T-096,100     |
| AC-013-10 | Dado dado remoto contendo instrução para exfiltrar segredo/alterar destino, quando modelo gera proposta, então backend nega ação fora de schema/escopo e publicação não ocorre sem aprovação vinculada.                                                         | FR-11,20          | T-097,100     |
| AC-013-11 | Dado bridge instalado e cliente pareado, quando Codex externo lista ferramentas e consulta, então recebe resultados limitados; na fase 1 não anuncia proposta/publicação; na fase 2 propõe publicação que só o app aprova; app ausente/revogação falha fechado. | FR-15..19         | T-099,100     |
| AC-013-12 | Dado app en/pt-BR, quando conecto, testo, publico, nego ou falha, então estados e orientação são localizados e navegáveis por teclado.                                                                                                                          | FR-21,22          | T-098,100     |
| AC-013-13 | Dado provider atual de texto/resumo, quando uso F009/F012 sem conector, então isolamento e comportamento anterior permanecem; importar MCP pessoal não ocorre.                                                                                                  | FR-17             | T-097,100     |
| AC-013-14 | Dado nenhum provider IA, quando exporto manualmente, então reviso/publico normalmente; quando peço proposta IA, então falta de provider é informada sem trocar automaticamente.                                                                                 | FR-09,10,18       | T-097,098     |
| AC-013-15 | Dado 429, paginação ou payload grande, quando consulto, então retries/bytes/deadline são limitados e truncamento é visível; criação não repete cegamente.                                                                                                       | NFR-02,03         | T-095,096,100 |
| AC-013-16 | Dada operação do cliente A e ID conhecido por B, quando B consulta status/proposta, então recebe PolicyDenied; revogar grant de A impede novas consultas MCP.                                                                                                   | FR-16             | T-099,100     |
| AC-013-29 | Dada conexão autorizada, quando peço ao assistente dados do Azure/Notion por comando explícito, então o backend executa leitura dentro do escopo e a resposta cita origem; destino fora do escopo retorna orientação, nunca conteúdo.                           | FR-06,20,32       | T-104,100     |
| AC-013-30 | Dado prompt pedindo ao assistente criar/alterar item, quando a mensagem é processada, então nenhuma escrita remota ocorre e o assistente orienta a revisão da reunião; não existe tool de escrita exposta ao modelo nem ao contexto.                            | FR-11,33          | T-104,100     |

Critérios OAuth **AC-013-17..20** e jornada de persistência estão em [oauth.md](oauth.md), com implementação T-094..T-100.

Na tabela, `FR-xx` abrevia `FR-013-xx`. Critérios autenticados usam workspace/projeto de teste e credenciais fornecidas no momento da validação, nunca em fixtures versionadas.

## Decisões abertas com proposta padrão

- **[NEEDS CLARIFICATION] Release e audiência:** proposta v1.1+ com OAuth individual, conexões por usuário. Definir registro Entra multitenant e callback Notion de produção antes da distribuição.
- **Decisão confirmada — casos de publicação e ordem:** fase 1 configura/instala MCPs com padrões; fase 2 integra o resumo do Notetaker, página Notion e sugestões Azure de Task/Feature/item de backlog, com escolha de backlog/pai/sprint. Ver [notetaker.md](notetaker.md).
- **[NEEDS CLARIFICATION] MCP no Codex interno:** proposta ficar com contexto/propostas no P0 e MCP externo na fase 1; somente avançar após spike de compatibilidade, sem mexer no provider atual.
- **[NEEDS CLARIFICATION] Hosts corporativos:** proposta Azure DevOps Services público; Server/on-premises e domínios personalizados ficam P2.

OAuth, jornadas de Notetaker e ordem das duas fases foram confirmados pelo usuário. Release, callback/empacotamento e variante técnica Azure permanecem sujeitos aos gates; não antecipar automaticamente o release v1.
