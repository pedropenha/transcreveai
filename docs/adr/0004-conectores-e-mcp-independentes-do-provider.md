# ADR-0004: Conectores com núcleo local e fachada MCP independente do provider

**Date**: 2026-10-05
**Status**: proposed
**Deciders**: usuário (solicitação), aprovação de arquitetura pendente

## Context

Notion e Azure DevOps precisam funcionar no Transcreve.ai com autorização OAuth por usuário e com Codex CLI ou BYOK. Hoje os providers são somente texto e CLI isolado. Acoplar conectores à configuração pessoal do Codex impediria controle comum e introduziria diferença de comportamento entre providers. Pesquisa e contratos estão em [F013](../../specs/features/013-connectors/spec.md).

## Decision

Propor núcleo Rust com adapter cliente MCP OAuth do Notion e REST com OAuth Entra para Azure DevOps, com segredos, escopo, aprovação e rede governados pelo backend. Usuário definiu duas fases: primeiro disponibilização/instalação pertinente e configuração MCP/OAuth com defaults; depois ações do resumo do Notetaker, publicação/vínculo Notion e sugestões Azure revisadas com backlog/pai/sprint. [Jornadas](../../specs/features/013-connectors/notetaker.md). Uma fachada MCP stdio instalada/configurada na fase 1 reutiliza o núcleo por IPC pareado ao app, fornecendo leitura na fase 1 e criação de rascunhos na fase 2; publicação exige aprovação no app.

Usuário escolheu OAuth como jornada principal. Azure usa Entra desktop public client; Notion usa MCP oficial hospedado com OAuth/PKCE, sujeito ao gate de callback de produção. [OAuth e persistência](../../specs/features/013-connectors/oauth.md) governa lifecycle. PAT continua alternativa tecnicamente possível, fora da jornada principal. Não instalar MCP de terceiros automaticamente nem alterar configuração pessoal do Codex.

## Alternatives Considered

### Embutir MCPs locais de terceiros de ambos os serviços

- **Pros**: capacidades prontas, protocolo comum, Microsoft oferece servidor local com PAT.
- **Cons**: dois lifecycles/runtimes, política e aprovação precisam de wrapper; MCP local Notion sem manutenção ativa; hospedados usam autenticações distintas.
- **Why not**: para o subset inicial, menor dependência operacional usando infraestrutura Rust existente. Reavaliar Azure local se spike demonstrar vantagem concreta.

### Configurar tudo exclusivamente no Codex CLI

- **Pros**: fácil para uso pessoal interativo.
- **Cons**: configuração ignorada pelo adapter atual; dependência de cliente/provider e aprovação headless; não resolve BYOK.
- **Why not**: incompatível com a intenção de conexões independentes da IA.

### Servidor MCP HTTP público próprio desde o início

- **Pros**: clientes cloud alcançam o serviço.
- **Cons**: hosting, identidade/autorização, isolamento de usuários, maior mudança na privacidade do produto.
- **Why not**: desnecessário para desktop local e BYOK com host local.

### REST com OAuth para ambos os serviços

- **Pros**: mantém contratos HTTP diretos.
- **Cons**: OAuth REST público do Notion exige proteger segredo de aplicação em backend confiável.
- **Why not**: preferir Notion MCP hospedado OAuth para evitar segredo global no desktop; validar callback de produção antes de prometer ausência de backend próprio.

## Consequences

### Positive

- Credenciais e política comuns ao app, Codex e BYOK; publicar sem IA é possível.
- Isolamento de texto/resumos preservado; modelo não aprova própria ação.
- Reuso de HTTP/cofre e SDK MCP oficial, sem implementar protocolo próprio.

### Negative

- Subset de API precisa de manutenção, mapeamentos e testes de contrato.
- Bridge depende do app ativo; stdio não atende diretamente cliente cloud.
- Mapping de ferramentas MCP Notion depende do catálogo/versão e deve falhar fechado em capacidades ausentes.

### Risks

- Duplicação após timeout: journal, estado desconhecido e reconciliação antes de repetir.
- Prompt injection e excesso de privilégios: escopo validado no backend e aprovação vinculada.
- IPC/cliente local: pareamento, ACL, grants revogáveis e gate de segurança.
- Escopo constitucional/release: registrar exceções aprovadas antes da implementação, sem alterar a v1 aceita por inferência.

## Approval gate

Solicitação autoriza esta especificação. Escolha final de release, distribuição pessoal vs pública e entrada MCP interna fica registrada como proposta em F013. Implementação deve atualizar a hierarquia de produto explicitamente; este ADR ainda não está accepted.
