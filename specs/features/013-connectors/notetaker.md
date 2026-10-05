# F013 — Duas fases: configurar MCPs e agir sobre o resumo do Notetaker

**Decisão de produto do usuário**: 2026-10-05 · **Status**: especificado, não implementado.

Esta direção substitui a sequência anterior que começava pela exportação e deixava toda configuração MCP para depois. OAuth continua sendo a jornada principal. A arquitetura de autenticação permanece sujeita aos gates de [oauth.md](oauth.md), sem pressupor que o Codex herda MCPs pessoais.

## Fase 1 — Disponibilização, conexão e configuração dos MCPs

Em Configurações → Conectores / MCP, disponibilizar Notion e Azure DevOps, permitir conectar a conta pelo navegador, testar acesso e persistir configuração e sessão OAuth.

MCP hospedado, como o Notion, é adicionado e conectado; não instala servidor na máquina. O MCP local do Transcreve.ai é distribuído/instalado com o app e configurado por caminho verificado. Azure pode usar esse MCP sobre o adapter Entra/REST para preservar compatibilidade e política comum; adoção do MCP Microsoft local/remoto depende do spike OAuth/cliente, não implica instalar pacote arbitrário ou usar PAT. A interface explica apenas os passos necessários, sem exigir Node/terminal para a jornada normal.

Esta fase entrega conexões, configuração de destinos e uso de leitura do MCP. Não publica reuniões nem cria work items. O gate demonstra MCP utilizável, não apenas arquivo de configuração gerado. A fachada MCP deve ter instalação, versão, diagnóstico e configuração exportável verificados com cliente externo; uso pelo app é gerenciado no backend e funciona com Codex ou BYOK por contexto, sem configuração manual no provider.

### Padrões de destino

Notion: conexão/workspace e página padrão opcional.

Azure: conexão/organização, projeto, equipe, backlog/nível e Area Path padrão; tipo padrão quando houver opção válida; política de sprint: **perguntar a cada criação** (inicial), **sprint específica** ou **sprint atual da equipe**. Usuário pode escolher qualquer projeto/equipe/sprint acessível e permitido. Sprints são carregadas da equipe/projeto, com ID, path e datas; nunca inventadas pela IA.

Sprint específica continua sendo a mesma até usuário alterá-la, mesmo quando termina; a revisão avisa sobre datas. Sprint atual é resolvida novamente ao preparar cada proposta, não fica congelada no ID da configuração. Se nenhuma ou mais de uma sprint corresponder, perguntar; não selecionar arbitrariamente. Seleção "Sem sprint" usa iteração de backlog válida para o contexto, não um path fictício.

Defaults são por conexão e contexto de projeto/equipe, persistidos sem segredos. Projeto/equipe alterados invalidam seleções dependentes; destino removido ou acesso perdido exige nova escolha. Alterar escolha no modal da reunião afeta apenas aquela proposta; atualizar padrão exige ação explícita.

## Fase 2 — Ações no resumo da reunião

### Notion: publicar e vincular

Na aba Resumo da janela de reunião e no detalhe de reunião do Hub, mostrar **Enviar ao Notion**. Abrir revisão com conexão, página de destino (padrão preenchido, alterável), título e resumo atual editável. Conteúdo adicional, como Minhas notas ou transcrição, só entra mediante seleção explícita. Nunca enviar áudio nessa ação.

Permitir selecionar página existente e **adicionar uma seção de resumo**, preservando conteúdo anterior, ou **criar subpágina** sob a página escolhida. Padrão proposto: subpágina, para manter uma reunião por página. Se o MCP não suportar uma operação, desabilitá-la com orientação; não substituir append por replace. Essa é uma ampliação explícita do escopo anterior que permitia apenas criação.

Após confirmação e sucesso, salvar vínculo local: reunião, versão/hash do resumo publicado, conexão, página destino e página resultante, URL verificada, operation_id e data. Mostrar **Abrir no Notion** e status de publicação. Fechar/reabrir/reiniciar preserva o vínculo. Editar/regenerar resumo informa que a versão publicada ficou desatualizada; não republicar/sincronizar automaticamente.

Permitir vincular somente a uma página existente sem enviar conteúdo, com rótulo que distingue **vinculado** de **publicado**. Publicação repetida oferece revisar o vínculo existente ou preparar novo envio explícito; duplo clique não duplica. Desconectar não apaga referência histórica, mas impede novas consultas/publicações até conexão válida.

### Azure DevOps: transformar próximos passos em itens

Na mesma aba/detalhe, mostrar **Criar item no Azure DevOps** (com Task quando esse tipo for suportado) e **Sugerir itens com IA**. A IA analisa o resumo atual e devolve cartões estruturados: título, descrição, tipo sugerido, motivo/trecho de origem e campos propostos. Pode sugerir Task, Feature ou item de backlog (Product Backlog Item, User Story, Issue ou equivalente do processo), sem fixar nomes universais.

Cada cartão tem ação **Revisar e criar**. O usuário escolhe conexão, projeto, equipe/backlog, tipo, pai opcional e sprint; defaults da fase 1 são preenchidos e sempre visíveis/editáveis. É possível aceitar, editar ou descartar cada sugestão e criar um item manualmente sem IA. Sugestão não confirma publicação.

"Backlog" tem dois sentidos distintos na UI: a lista/nível da equipe onde o item aparecerá e o tipo de item de backlog. Para relacionar Task a um item existente, escolher pai por consulta; para relacionar item de backlog a Feature existente, escolher Feature por consulta. IDs e opções precisam existir e pertencer ao projeto/escopo. Nome parecido não basta.

A IA pode sugerir equipe/backlog/pai entre candidatos realmente consultados e autorizados, com justificativa e escolha explícita do usuário. Não cria projeto, equipe, backlog, sprint ou hierarquia completa silenciosamente. Se for útil criar uma Feature e depois filhos, aprovar/criar uma proposta por vez, resolver o ID confirmado do pai e revisar cada filho; sem transação remota presumida.

Task direta pode não aparecer no product backlog: respeitar tipo/categoria, Area Path, configuração de equipe e Iteration Path. Quando for necessário pai ou houver visibilidade distinta, informar durante revisão. Sprint e backlog não são campos livres inventados pelo modelo. Feature pode abranger várias sprints; sprint só é aplicada quando o schema/contexto suportar e o usuário confirmar.

Após sucesso, mostrar ID/link e persistir vínculo à reunião/sugestão, payload fingerprint e contexto de destino. Regenerar resumo/sugestões mantém referências de itens já criados; não criar novamente automaticamente. Sem resumo válido, sem conector conectado ou sem acesso, informar o passo necessário. Falha da IA não remove o resumo nem impede criação manual.

## Requisitos adicionais

- **FR-013-24** Entregar fase 1 completa (disponibilização/instalação pertinente, OAuth, diagnóstico, configuração, leitura MCP e persistência) antes de habilitar ações da fase 2. Instalação gerenciada é escolha explícita do usuário, sem executar ações externas nesta especificação.
- **FR-013-25** Configurar e persistir defaults Azure por conexão/projeto/equipe, inclusive backlog/Area Path e política de sprint. Descobrir opções reais com escopo; validar dependências ao salvar e ao preparar/publicar.
- **FR-013-26** Aplicar default somente como preenchimento revisável; overrides de operação não alteram configuração. Resolver sprint atual no preparo e verificar existência/acesso antes do envio; qualquer mudança de destino invalida aprovação.
- **FR-013-27** Ações do Notion disponíveis no resumo do Notetaker: escolher/vincular página, append seguro ou subpágina, revisar conteúdo e publicar explicitamente. Schema ausente é UnsupportedCapability, nunca sobrescrita substituta.
- **FR-013-28** Ações Azure disponíveis no resumo: criação manual e sugestões estruturadas pela IA, por item, de Task/Feature/item de backlog e destino/pai existentes. Usar provider já escolhido e validar todo resultado no backend.
- **FR-013-29** Revisão mostra projeto, equipe/backlog, tipo, pai, Area Path, sprint/Iteration Path, campos obrigatórios e conteúdo; permitir alterar antes de confirmação. Aplicar processo real, hierarquia e visibilidade sem assumir que criar Task equivale a entrar em product backlog.
- **FR-013-30** Persistir vínculo por reunião/versão de resumo/operation_id ao Notion ou Azure; reabrir mantém links, editar/regenerar indica divergência sem replay; desconectar não remove referência histórica. IDs/vínculos da reunião A não contaminam reunião B.
- **FR-013-31** Distinguir vinculada/publicada/sugerida/criada/falha/resultado desconhecido na interface. Aprovação, journal, offline, reconciliação e i18n continuam conforme F013; publicação parcial não apaga sugestões restantes.

## Aceitação por fase

| ID        | Dado / Quando / Então                                                                                                                                                                                               | Refs        | Tarefas                    |
| --------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ----------- | -------------------------- |
| AC-013-21 | Dado instalador e conta autorizada, quando concluo fase 1 e reinicio, então MCPs/configuração/leitura funcionam, defaults reaparecem e ações de publicação ainda não estão habilitadas antes do gate da fase 2.     | FR-24,25    | T-094..096,099,101,102,100 |
| AC-013-22 | Dado projeto/equipe padrão, quando seleciono sprint específica, atual ou perguntar, então preparo preenche a opção correspondente; mudar projeto/equipe limpa dependências inválidas e override não muda padrão.    | FR-25,26    | T-102,097,100              |
| AC-013-23 | Dado resumo editado, quando envio ao Notion para página escolhida, então preview corresponde ao snapshot aprovado, append preserva conteúdo ou subpágina é criada; vínculo/link persiste e regenerar não republica. | FR-27,30,31 | T-095,097,098,103,100      |
| AC-013-24 | Dado processo Azure real, quando peço sugestões, então cartões oferecem somente tipos/candidatos válidos, motivo e origem; usuário escolhe backlog/pai/sprint e só item aprovado é criado.                          | FR-28,29    | T-096,097,098,102,100      |
| AC-013-25 | Dado destino/sprint removido, acesso perdido ou pai fora do projeto, quando preparo/publico, então ação exige corrigir destino sem fallback silencioso ou ampliar acesso.                                           | FR-25,26,29 | T-096,097,102,100          |
| AC-013-26 | Dado resumo com itens já publicados, quando reabro ou regenero, então links/IDs continuam visíveis, não se cria duplicata e sugestões/estados são próprios daquela reunião.                                         | FR-30,31    | T-097,098,103,100          |
| AC-013-27 | Dado página existente, quando apenas vinculo sem publicar, então nenhuma escrita remota ocorre e UI mostra vinculado, sem afirmar que o resumo foi enviado.                                                         | FR-27,31    | T-098,103,100              |
| AC-013-28 | Dado Codex ou BYOK selecionado, quando sugiro itens no resumo, então ambos usam o mesmo contexto/defaults e política; sem IA posso criar manualmente e nenhum token é entregue ao modelo.                           | FR-28,29    | T-097,098,100              |

Gate fase 1: AC-013-01,02,05..07,17..22 e parte de instalação/pareamento/leitura/revogação de AC-013-11. Validar isolamento de conexões/grants; ownership de propostas (AC-013-16) e parte proposta/status/aprovação de AC-013-11 ficam na fase 2. Exigir validação OAuth/MCP real. Disponibilização e aceite de leitura são obrigatórios; usuário pode optar por não usar cliente externo. Gate fase 2: todos os AC-013-01..28 pertinentes à UI e publicação, com destinos de teste autenticados. Não marcar fase inteira concluída por documentação, mocks ou arquivo de configuração.

## Referências de domínio pesquisadas

Azure Boards diferencia níveis de backlog, tipos de item e hierarquia; visibilidade depende da equipe/área e sprint usa iteração. A especificação aplica isso aos seletores, sem inventar um campo único "backlog" na REST. [Visão de backlogs](https://learn.microsoft.com/en-us/azure/devops/boards/backlogs/backlogs-overview?view=azure-devops), [configuração de Boards](https://learn.microsoft.com/en-us/azure/devops/boards/configure-customize?view=azure-devops).

MCP Microsoft possui ferramentas de iterações/backlogs, mas nomes e schemas precisam ser fixados após testar a versão selecionada. [Ferramentas de Work](https://github.com/microsoft/azure-devops-mcp/blob/main/src/tools/work.ts), [exemplos oficiais](https://github.com/microsoft/azure-devops-mcp/blob/main/docs/EXAMPLES.md).
