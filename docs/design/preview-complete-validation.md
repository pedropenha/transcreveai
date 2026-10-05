# Proposta completa — Papel & Anil / Vidro & Anil

Atualizada em 2026-10-03, em `docs/design/design-preview.html`. Protótipo HTML navegável com dados ilustrativos; não altera o app instalado nem executa áudio, tradução, agentes ou colagem. Fontes locais do projeto, sem Google Fonts.

## Conteúdo

- Início/histórico e estatísticas; Notetaker com seleção e filtro de reuniões.
- Dicionário com vocabulário/muletas editáveis somente na prévia; substituições futuras e ausência de aprendizado automático explícitas.
- Configurações com categorias, modelos, agentes CLI, retenção e controles ilustrativos.
- Assistente Vidro & Anil com compositor, conversa ilustrativa, nova conversa e estado de fixação; proposta detalhada do painel vinculada.
- Flow Bar com estados ilustrativos; boas-vindas sem prática obrigatória.
- Comando de tradução português → inglês por sessão: proposta T-093, sem implementação no app. Modelo compatível obrigatório; sem fallback silencioso para nuvem.
- Ajuda com privacidade, offline, limites de acessibilidade e escopo atualizado.

## Verificação

- Chromium/Playwright: oito telas em claro/escuro, sem erros JavaScript.
- Axe: 16 cenários sem violações nas regras WCAG 2 A/AA, 2.1 AA e 2.2 AA verificadas; cenário adicional Privacidade também sem violações. Não equivale a auditoria manual ou declaração integral de conformidade.
- Interações: grupos do dicionário, substituições não editáveis, envio seguro de texto literal no compositor, nova conversa e conclusão das quatro etapas de boas-vindas.
- Configurar agente abre a categoria correta; retorno a Geral mantém um único marcador de categoria ativa.
- Oito telas a 900 px sem transbordamento horizontal. Capturas de Início, Comandos, Assistente e Privacidade em `docs/design/screens/preview-complete-*`.
- Revisão independente identificou textos antigos sobre chave de resumo, escopo da Flow Bar e estados de navegação; corrigidos.

## Limites

Os controles são demonstrações e não persistem no app. Não houve nova validação do backend, de gravação/colagem nativa, de leitores de tela, de tradução real ou do instalador nesta tarefa. T-093 e exclusão unificada da T-046 continuam pendentes. A aceitação manual da T-044 permanece com o usuário.
