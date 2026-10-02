# F010 — Hub, histórico, configurações, onboarding e bandeja

**Status**: Draft · **Release**: v1 (histórico, reuniões, dicionário, modelos, configurações, onboarding, bandeja) · v1.1+ (notas, snippets, estilos)

## Contexto

O Hub é a janela principal: onde o usuário vê o que ditou, gerencia notas e reuniões e configura tudo. Deve ser rápido, navegável por teclado e bilíngue (pt-BR/en).

## Estrutura do Hub (barra lateral)

O rail fica sobre o canvas (papel) e o conteúdo vive num painel inset arredondado; só o painel rola. Largura 236 px, colapsável para 64 px (só ícones + tooltip; a escolha persiste por usuário no navegador). A barra de título nativa é mantida ([ADR-0003](../../../docs/adr/0003-identidade-visual-papel-e-anil.md)).

| Grupo / seção               | Release                       | Conteúdo                                                                      |
| --------------------------- | ----------------------------- | ----------------------------------------------------------------------------- |
| **Início**                  | v1                            | Histórico de ditados + estatísticas                                           |
| **Notetaker** (reuniões)    | v1                            | Lista e detalhes (F009)                                                       |
| **Notas**                   | v1.1+ (visível, desabilitada) | Scratchpad (F007); item com tag "v1.1", sem navegação                         |
| _Personalizar_ · Dicionário | v1 (vocab + muletas) / v1.1+  | Termos, muletas e substituições                                               |
| _Personalizar_ · Assistente | v1.1+ (visível, desabilitada) | Painel do assistente (F012); item com tag "v1.1", sem navegação               |
| Snippets / Estilos          | v1.1+                         | Gatilhos e expansões; perfis de app (ainda sem item no rail)                  |
| Rodapé · **Configurações**  | v1                            | Sub-navegação por categorias: Uso, Transcrição, Inteligência, App (FR-010-27) |
| Rodapé · **Ajuda**          | v1                            | Atalhos de teclado e sobre o app                                              |

**Modelos & Provedores não é mais uma seção do rail**: vive em Configurações → Modelos (F003; provedores em nuvem na v1.1+). O deep link antigo `hub://navigate { section: "models" }` continua funcionando e leva a Configurações → Modelos.

### Navegação e checklist

- **FR-010-20** Atalhos: `Ctrl/Cmd+1…5` trocam para Início, Notetaker, Dicionário, Configurações e Ajuda (itens v1.1+ são pulados); `Ctrl/Cmd+,` abre Configurações.
- **FR-010-21** Card "Configurar Notetaker" no pé do rail com 4 itens (baixar modelo, testar microfone, permitir áudio do sistema, chave para resumos), barra de progresso e deep link por item para a aba de Configurações correspondente. Itens de modelo e de provedor de resumo são derivados do estado real; os demais são marcados ao serem visitados. O card some ao concluir tudo ou ao ser dispensado; o estado persiste no setting `dismissed_ui` (lista de ids; ids `setup_checklist` e `setup:<item>`).
- **FR-010-22** `dismissed_ui` é saneado no backend (trim, sem duplicatas/vazios, ids ≤ 64 caracteres, no máximo 32 ids).

## Requisitos funcionais

### Início / Histórico

- **FR-010-01** Lista cronológica (agrupada por dia) com: hora, ícone/nome do app, texto final (2 linhas), modo (ditado/comando/nota), status (ícone para falha/copiado).
- **FR-010-02** Busca full-text; filtros por app, modo, status, período.
- **FR-010-03** Detalhe da entrada: texto cru × final (diff), provedor/modelo, idioma, latências (STT/LLM/inserção), estágios do pipeline aplicados.
- **FR-010-04** Ações: copiar, reinserir no app em foco, **tentar novamente** (se o áudio ainda existir, escolhendo o provedor), adicionar palavra ao dicionário a partir de um trecho selecionado, sinalizar, excluir.
- **FR-010-05** Estatísticas: palavras ditadas (hoje/semana/total), velocidade média (palavras por minuto de fala), tempo economizado estimado (vs. 40 ppm digitando), minutos e custo estimado por provedor (F003).
- **FR-010-23** Layout do Início: saudação por horário ("Bom dia/Boa tarde/Boa noite") e botões Atalhos e Gravações; banner dispensável ("Sua voz, seu computador", com botão que leva a Configurações → Modelos); barra de busca e filtros; histórico agrupado por dia (Hoje, Ontem, depois data por extenso), virtualizado; coluna lateral de estatísticas e dica "Mãos livres" dispensável. Banner e dica persistem a dispensa em `dismissed_ui` (`home_banner`, `home_tip`). Abaixo de ~900 px de painel a coluna lateral vira faixa no topo.
- **FR-010-24** Linha do histórico: hora, texto (até 3 linhas), app de origem, nº de palavras e chips de estado; ações (copiar, sinalizar, mais detalhes) aparecem no hover **e** no foco por teclado; falhas mostram o chip "Falhou" e "Tentar novamente" (quando o áudio existe). "Mais detalhes" abre o painel de detalhe (FR-010-03/04) com Esc para fechar. Ouvir o áudio da entrada fica para a v1.1+ (sem comando de reprodução no backend).
- **FR-010-25** Card de estatísticas: palavras totais, ppm, dias seguidos, tempo economizado (vs. 40 ppm) e modelo ativo. A sequência é calculada no frontend sobre as entradas mais recentes **sem filtros** (até 5 páginas de 100, parando quando a sequência não pode mais crescer); acima de ~500 ditados contíguos ela é subestimada até o backend expor o dado.
- **FR-010-26** Layout do Notetaker (etapa 4): ver FR-009-29. O Início e o Notetaker rolam por conta própria (lista e detalhes rolam separados); as demais seções rolam o painel inteiro.
- **FR-010-27** Configurações com sub-navegação própria (coluna à esquerda; abaixo de ~760 px de painel vira uma faixa horizontal no topo) e migalha "Configurações › Categoria". Páginas: **Uso** (Geral — aparência e idioma da interface; Atalhos; Microfone e sons) · **Transcrição** (Modelos; Idiomas; API com chave própria) · **Inteligência** (Resumos de reunião; Assistente) · **App** (Sistema; Privacidade; Avançado). O deep link `settingsTab` (`hub://navigate`, checklist, toasts, painel do assistente, banner do Início) aceita o caminho `categoria/página` (ex.: `transcription/models`, `intelligence/summaries`), uma categoria sozinha (cai na primeira página) e os nomes planos antigos (`general`, `models`, `system`, `privacy`, `advanced`); valores desconhecidos são ignorados. Destinos: banner/checklist de modelo → `transcription/models`; checklist de microfone → `usage/audio`; áudio do sistema → `app/privacy`; chave de resumos e toast "abrir configurações de resumo" → `intelligence/summaries`; painel do assistente → `intelligence/assistant`.
- **FR-010-06** Navegação por teclado (↑/↓ entre entradas, Enter abre, Ctrl+C copia) e compatível com leitor de tela.

### Configurações

- **FR-010-07** **Geral**: atalhos (captura + conflitos, F002), microfone (com medidor de nível ao vivo e teste "grave 3 s e ouça"), idiomas de ditado, idioma da interface, sons.
- **FR-010-08** **Sistema**: iniciar com o Windows (padrão ligado), visibilidade e comportamento da Flow Bar (F001), ícone na bandeja, posição dos toasts.
- **FR-010-09** **Privacidade** (F011): modo offline, retenção de áudio e histórico, enviar título da janela ao LLM, "Apagar todos os dados".
- **FR-010-10** **Avançado**: atrasos de colagem, método de inserção padrão, pré-carregar modelo, descarregar modelo após N min, modo debug de logs, abrir pasta de dados.
- **FR-010-11** P2 — exportar/importar configurações, dicionário, snippets e perfis (JSON, **sem segredos**).
- **FR-010-12** Mudanças aplicadas imediatamente (sem "Salvar"), com validação inline.

### Bandeja

- **FR-010-13** Ícone com estados: normal, gravando (vermelho), erro (âmbar), offline (indicador).
- **FR-010-14** Menu: Abrir Hub · Iniciar/Parar ditado · Iniciar/Parar reunião · Mostrar/Ocultar Flow Bar · Pausar detecção de reuniões por 1 h · Modo offline · Sair. (v1.1+: "Nova nota por voz", F007.)
- **FR-010-15** Fechar o Hub minimiza para a bandeja (o app continua rodando); "Sair" encerra de fato (confirmando se houver gravação ativa).

### Onboarding (primeira execução)

- **FR-010-16** Passos:
  1. Boas-vindas + idioma da interface e de ditado.
  2. **Microfone**: verifica permissão do Windows ("Permitir que apps da área de trabalho acessem o microfone"); se negada, botão que abre `ms-settings:privacy-microphone`; seleção de dispositivo com medidor.
  3. **Transcrição**: escolha do **modelo local** — `large-v3-turbo` marcado como recomendado, rótulos de adequação por hardware (F003), baixa com progresso e dá para seguir enquanto baixa. (v1.1+: opção _Nuvem_ com chave própria.)
  4. **Atalho**: mostra o padrão, permite trocar, e um campo de prática: "Segure `Ctrl+Win` e diga: _Olá, estou testando o Transcreve.ai_" — sucesso quando o texto aparece no campo.
  5. **Flow Bar**: animação mostrando hover e as 2 ações.
  6. **Reuniões**: liga/desliga detecção, explica consentimento.
- **FR-010-17** Onboarding pode ser pulado e reaberto em Configurações → Ajuda.
- **FR-010-18** Instância única: abrir o app de novo foca o Hub existente.
- **FR-010-19** (v1.1+, com a T-049) Atualizações: checagem diária (desativável), notas da versão, instalar ao sair. Na v1 não há updater.

## Requisitos não funcionais

- **NFR-010-01** Hub abre em ≤ 500 ms (janela já criada e oculta).
- **NFR-010-02** Histórico com 100.000 entradas: rolagem fluida (lista virtualizada) e busca ≤ 150 ms.
- **NFR-010-03** Todas as strings via i18n (pt-BR, en), mantidas com a skill `i18n-sync`.
- **NFR-010-04** Acessibilidade conforme as skills `accessibility`/`frontend-a11y` e checagens axe de `rules/react/testing.md`.
- **NFR-010-05** O visual do Hub segue a direção visual **Papel & Anil** ([ADR-0003](../../../docs/adr/0003-identidade-visual-papel-e-anil.md), proposta em `docs/design/proposta-ui.html`; `rules/web/design-quality.md`), mantendo a organização de seções do Wispr. Contraste WCAG 2.2 AA dos tokens é verificado por teste unitário.

## Critérios de aceitação

- **AC-010-01** _Dado_ uma instalação nova, _quando_ completo o onboarding escolhendo um modelo local, _então_ o campo de prática recebe o texto ditado e o onboarding marca sucesso.
- **AC-010-02** _Dado_ o mic bloqueado nas configurações de privacidade do Windows, _então_ o passo 2 explica e o botão abre a página correta.
- **AC-010-03** _Quando_ clico em "Tentar novamente" numa entrada que falhou, escolhendo outro provedor, _então_ uma nova transcrição é feita e a entrada é atualizada.
- **AC-010-05** _Quando_ fecho o Hub, _então_ o app continua na bandeja e os atalhos seguem funcionando.
- **AC-010-06** _Dado_ uma gravação de reunião ativa, _quando_ escolho "Sair", _então_ sou avisado e posso cancelar.
