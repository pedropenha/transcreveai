# F013 — OAuth e persistência de conexões

**Direção solicitada pelo usuário**: 2026-10-05, OAuth em vez de entrada de PAT. **Status técnico**: proposta, sem implementação nem autenticação real testada. Este documento complementa spec/plan e governa autenticação e persistência.

PAT é tecnicamente possível pelas APIs oficiais; o motivo da mudança é a experiência de conexão por conta e a compatibilidade com serviços hospedados. Não atribuir a decisão a impossibilidade técnica de PAT.

## Escolha por serviço

| Serviço                          | Caminho proposto                                                                                          | Infraestrutura e dificuldade                                                                                                                  |
| -------------------------------- | --------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------- |
| Notion                           | App como cliente do MCP oficial `https://mcp.notion.com/mcp`, OAuth authorization code + PKCE S256        | Moderada: discovery, registro de cliente, callback, renovação e mapeamento das ferramentas. Não exige hospedar o MCP                          |
| Azure DevOps Services            | REST oficial com token delegado Microsoft Entra; desktop public client com authorization code + PKCE S256 | Moderada: app registration, redirects, tenant/consentimento e cache de tokens. Sem client_secret global no desktop                            |
| Notion REST público, alternativa | OAuth público com troca/refresh usando client_secret de aplicação                                         | Exige broker/backend confiável para app distribuído. Não embutir segredo de aplicação no instalador; não adotar silenciosamente se MCP falhar |

Avaliação de dificuldade é inferência, não estimativa de prazo. A integração OAuth completa inclui UX de reconexão e teste de permissões reais; persistência isoladamente é pequena extensão do cofre existente.

Notion documenta cliente MCP em Rust com SDK oficial e crate `oauth2`, PKCE, registro dinâmico e alternativa CIMD. O guia documenta cliente público com auth method none e PKCE, mas orienta HTTPS para redirects de produção e HTTP localhost para desenvolvimento. Portanto, callback desktop empacotado é gate: não prometer ausência de backend próprio até validá-lo. Confirmar modo public-client/redirect suportado no spike; persistir registro por instalação, sem registrar um cliente novo a cada abertura. Segredo eventualmente emitido por DCR é por instalação e vai ao cofre; não é um client_secret global embutido. [Cliente MCP Notion](https://developers.notion.com/guides/mcp/build-mcp-client).

Azure: registrar aplicação Entra para desktop e validar audiência/tenant e permissão delegada para Azure DevOps. Pedir `offline_access` quando necessário ao fluxo de refresh, sem confundir com o modo offline do app. OAuth legado específico de Azure DevOps não é base nova. Compatibilidade do MCP remoto Azure com um cliente não impede usar token Entra diretamente na REST. [Orientação Microsoft](https://learn.microsoft.com/en-us/azure/devops/integrate/get-started/authentication/authentication-guidance?view=azure-devops), [fluxo PKCE](https://learn.microsoft.com/en-us/entra/identity-platform/v2-oauth2-auth-code-flow).

Decisão de UX (2026-10-05): o app embute um Application ID público multi-tenant mantido pelo projeto (`AZURE_DEVOPS_DEFAULT_CLIENT_ID` em `connectors/oauth.rs`) — client ID de cliente público não é segredo. O usuário conecta direto sem registrar nada; os campos Application ID/Tenant ficam como override avançado para organizações que preferem o próprio registro. Tenant padrão `common` (o registro aceita contas organizacionais e pessoais).

## Jornada

1. Usuário clica Conectar Notion ou Conectar Azure DevOps.
2. Backend cria tentativa de curta duração com state aleatório, PKCE e vínculo à conexão/issuer; abre navegador do sistema. Nunca pedir senha no app/WebView.
3. Usuário autentica e autoriza permissões no serviço. Tenant pode exigir aprovação de administrador; informar sem contornar política.
4. Callback valida state, tentativa, issuer, redirect e uso único; backend troca código por tokens. Recusa callback atrasado, duplicado ou de outra tentativa.
5. Persistir bundle no cofre antes de marcar conectado; validar identidade e mostrar escolha de escopo/destinos. Autenticação não implica escrita autorizada.
6. Ao reiniciar, carregar conexão e usar refresh quando necessário. Não abrir navegador automaticamente; se autorização terminar, mostrar Reconectar e aguardar ação do usuário.

Callback preferencial loopback com listener temporário apenas local, porta/redirect aceitos pela registration. Deadline proposto 5 min; validar path/Host, limitar tamanho, sem CORS aberto, encerrar no sucesso/cancelamento/timeout. Não é servidor MCP HTTP público. F011 FR-27 precisa permitir explicitamente essa exceção transitória e aplicar os controles de endpoints pertinentes. Se provedor não aceitar loopback, documentar alternativa e gate antes de implementação; não publicar listener em 0.0.0.0.

## Persistência e renovação

- Cofre: access token, refresh token, segredo DCR se existir e bundle versionado por grant. Nenhum comando Tauri lê token de volta; frontend recebe apenas estado/identidade mascarada. Backend é único dono de refresh; bridge e Codex não recebem tokens.
- Store: connection_id, serviço, workspace/tenant/account IDs, label, scope concedido e escolhido, auth_mode, client_id público, expires_at e referência do cofre. Sem tokens/code/state/PKCE verifier em settings, SQLite ou localStorage.
- State e verifier vivem só durante tentativa e são descartados ao terminar; crash exige iniciar conexão de novo. Callback URL com code nunca entra em log.
- Refresh por grant é serializado. Persistir token bundle novo de forma atômica antes de outra chamada; se cofre falhar, não reportar sucesso nem continuar rodando refresh concorrente.
- No Notion, refresh token rotaciona; guardar o novo e preservar a identidade de registro. `invalid_grant` é terminal: marcar reconexão necessária, interromper retry e solicitar autorização interativa pelo usuário. Não prometer sessão permanente.
- Falha transitória preserva bundle e rascunho, com backoff limitado. `invalid_client`, mudança de tenant/conta e política administrativa têm orientação específica; não tentar PAT ou provider diferente como fallback.
- Antes de mutação remota, verificar token/política/aprovação. Refresh de autenticação não autoriza repetir escrita cujo resultado é desconhecido.
- Desconectar: revogar localmente grants e propostas, apagar bundle; tentar revogação remota somente se endpoint e semântica forem suportados. Falha de revogação remota não impede exclusão local; indicar como remover acesso na conta do serviço. Offline não tenta refresh/revogação pela rede.

OAuth do Notion MCP não é OAuth da API REST; tokens são vinculados a issuer/audiência/resource. Nunca reutilizá-los em `api.notion.com` por suposição. Azure access token precisa ser emitido para Azure DevOps, não Graph. MCP/BYOK/Codex recebem ferramentas/contexto através do núcleo, não o bundle OAuth.

## Critérios adicionais

- **AC-013-17** Dada conexão autorizada e refresh válido, quando reinicio o app, então conexão é recuperada e renovada sem novo login; store/logs/argv/prompts não contêm tokens.
- **AC-013-18** Dadas duas chamadas simultâneas e token expirado, quando renovam, então apenas um refresh por grant ocorre e bundle rotacionado persiste antes do uso; `invalid_grant` gera Reconectar sem loop.
- **AC-013-19** Dado callback com state incorreto, replay, issuer errado ou tentativa cancelada, quando chega, então nenhum token é salvo e nenhuma conexão fica pronta; listener encerra no prazo.
- **AC-013-20** Dado desconectar/offline/cofre indisponível, quando refresh ou bridge tenta acessar, então respeita revogação/bloqueio e não publica automaticamente; segredo global de aplicação não está no instalador.

Refs: FR-013-02..05,16,21,23 e NFR-013-01,04,05; implementação de conexão/refresh na fase 1 (T-094..096, T-101, T-099), conforme [notetaker.md](notetaker.md). Registrar cenários autenticados em Windows e ambos providers separadamente; teste sintético de refresh não comprova a política do tenant/workspace.

## Fontes e restrições

Notion MCP hospedado fornece OAuth interativo; ligação cliente e lifecycle precisam ser validados na versão do SDK escolhida. O guia descreve persistência do registro, serialização e rotação; nenhuma sessão foi testada aqui. [Conexão oficial](https://developers.notion.com/guides/mcp/get-started-with-mcp), [cliente/lifecycle](https://developers.notion.com/guides/mcp/build-mcp-client).

REST público Notion exige credenciais de aplicação na troca/refresh, portanto não equivale ao fluxo public-client MCP. [OAuth REST](https://developers.notion.com/guides/get-started/authorization).
