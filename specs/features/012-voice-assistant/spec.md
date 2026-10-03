# F012 — Assistente por voz (overlay) + providers de agentes CLI

**Status**: Draft · **Release**: v1.1+ (antecipável) · **Depende de**: F001, F002, F003, T-050 (`LlmProvider`), T-016 (keyring)

## Contexto

O usuário quer conversar com uma IA a qualquer momento, **por voz**, sem abrir
navegador nem o Hub — direto de um overlay flutuante acionado por um atalho.
E, em vez de pagar API por token (BYOK), os providers podem ser os
**agentes CLI já instalados na máquina** — Codex, Claude Code, Devin, Cursor —
usando a assinatura que o usuário já tem.

## Escopo

**Dentro:**

- Providers de agentes CLI atrás do trait `LlmProvider` (T-050) — servem ao
  resumo de reunião (FR-009-16..22) e ao assistente.
- Overlay do assistente: aberto **somente por atalho global configurável** —
  sem slot na Flow Bar (decisão de UX: o notch fica com duas ações em pills
  separadas); o atalho abre o painel **já ditando** e a segunda pressão para
  o ditado e **envia**; resposta no painel; multi-turn dentro da sessão.
- Painel **arrastável e fixável** — independente da posição da Flow Bar.
- **Contexto local**: o assistente responde sobre ditados e reuniões
  recentes — um snapshot limitado (títulos, resumos, notas, trechos de
  transcrição) vai junto na chamada do provider.

**Fora (não fazer agora):**

- Janela de chat completa no Hub / histórico persistente de conversas.
- Campo de texto editável no painel — o painel é dictation-first: a
  transcrição ao vivo é o "rascunho", e o envio é a parada do ditado.
- Command Mode (F006) continua separado — lá a resposta é inserida como texto;
  aqui a resposta fica no overlay e o usuário decide o que fazer com ela.

## Histórias

- **US-012-01** Quero apertar um atalho em qualquer app, ditar "que horas é a
  reunião de amanhã no meu calendário" e ler a resposta num painel.
- **US-012-02** Quero usar meu Codex/Claude Code já logado, sem configurar
  API key.
- **US-012-03** Quero ditar e ver a transcrição ao vivo no painel — e poder
  cancelar antes de enviar (o envio é a segunda pressão do atalho).
- **US-012-04** Quero arrastar o painel para o canto da tela e fixá-lo ali,
  sobrevivendo a reinícios.
- **US-012-05** Quero gerar o resumo da reunião usando a assinatura do meu
  agente CLI em vez de uma chave de API.

## Requisitos funcionais

### Providers de agentes CLI

- **FR-012-01** Nova implementação `cli_agent` do trait `LlmProvider`, com
  adaptadores por ferramenta. Cada adaptador define: nome do binário, argv de
  modo não-interativo, formato de saída e flags de desativação de
  confirmações/permite writes desligadas (somente leitura/geração de texto).
  Alvos iniciais: `codex exec`, `claude -p`. `cursor-agent` e `devin`
  ficam **listados como experimentais** — sem modo não-mutante headless
  verificado (NFR-012-02). Eles **nunca** são auto-selecionados e são
  recusados nos usos silenciosos (resumo, limpeza), mas uma **seleção
  explícita como provider do assistente** é o opt-in que os libera — a UI
  os marca experimental e o painel sinaliza isso durante a chamada.
  Os flags exatos de `codex` e `claude` são confirmados na implementação
  contra a versão instalada.
- **FR-012-02** Detecção: o provider aparece na tela Modelos & Provedores como
  `detectado`/`ausente` conforme o binário estar no `PATH`. Ausente → linha
  desabilitada com dica de instalação.
- **FR-012-03** **Sem chave de API**: a autenticação é a sessão do próprio CLI
  (assinatura). Nada vai para o keyring.
- **FR-012-04** O roteador cost-aware (T-050) trata providers CLI como
  **custo zero**; o usuário escolhe o provider do resumo e do assistente nas
  configurações — a escolha do assistente é **explícita** (Codex, Claude,
  Cursor, Devin ou um BYOK); `auto` resolve pelo provider configurado ou o
  primeiro CLI detectado, e **nunca** escolhe um adaptador experimental.
- **FR-012-05** Por provider: habilitar/desabilitar, caminho do binário
  (override), args extras e timeout (padrão 60 s para assistente, 180 s para
  resumo map-reduce).

### Overlay do assistente

- **FR-012-10** Binding `assistant`: atalho global configurável (mesma tela de
  captura dos demais atalhos, F010) **da família de ditado** — a primeira
  pressão abre o painel e inicia a captura; a segunda encerra o ditado e
  envia. A Flow Bar **não** ganha botão do assistente (decisão de UX
  registrada em 2026-10).
- **FR-012-11** O atalho abre um painel overlay `topmost` que **não
  rouba foco** ao surgir; o foco vai ao painel só quando o usuário clicar/ditar
  (o ditado roteado pelo binding `assistant` é intenção explícita → foca).
- **FR-012-12** Só o binding `assistant` é roteado ao painel — `transcribe` /
  `Ctrl+Espaço` **sempre** ditam para o app focado, mesmo com o painel
  aberto. A transcrição ao vivo aparece no painel como bolha pendente —
  mesmo pipeline de STT do ditado, sem limpeza LLM. O texto **nunca é
  inserido em outros apps**; o destino é o painel.
- **FR-012-13** Envio: **a segunda pressão do atalho `assistant`** encerra o
  ditado e auto-envia a transcrição ao provider. Não há campo editável — o
  painel é dictation-first. Se um ditado terminar enquanto uma resposta está
  em andamento, o prompt é **enfileirado** e enviado quando o turno atual
  comita (nunca descartado, nunca interrompe a resposta em curso).
- **FR-012-14** Resposta renderizada no painel (Markdown leve: negrito,
  listas, código inline). Durante a chamada: estado `thinking` **com o nome
  do provider em uso** e botão de cancelar; erro do provider → estado de
  erro localizado com `Tentar de novo`.
- **FR-012-15** Multi-turn: as mensagens anteriores da sessão do painel entram
  na chamada (truncadas a um limite de tokens configurável). "Nova conversa"
  limpa o contexto.
- **FR-012-16** **Arrastar e fixar**: área de título do painel arrasta para
  qualquer ponto da tela; posição persistida e restaurada; toggle **Fixar**
  trava o painel (fica visível mas imóvel); respeita bordas e multi-monitor
  (se o monitor sumir, volta ao monitor primário).
- **FR-012-17** Sem provider LLM configurado/detectado → estado vazio no
  painel: "o assistente precisa de um provider de IA" + botão para abrir
  Configurações. Nenhuma chamada é feita.
- **FR-012-18** Privacidade: o painel mostra qual provider está ativo; nada
  sai da máquina até o envio explícito do usuário; transcrição é local.
- **FR-012-19** Contexto: no envio, o system prompt carrega um snapshot
  **limitado** dos ditados recentes e das reuniões (título, data, status,
  resumo, "Minhas notas" e trecho final da transcrição das mais novas) —
  para perguntas como "sobre o que eu falei ontem?". O bloco é capado
  (~24k chars), lido fora da thread de UI e **fail-open**: erro de leitura
  nunca bloqueia nem falha o turno. Conteúdo nunca é logado (T-003).

## Requisitos não-funcionais

- **NFR-012-01** Overlay abre em < 200 ms; transcrição ao vivo com a mesma
  latência do ditado; painel nunca bloqueia a sessão de ditado em andamento
  (coexistência como F009/FR-009-15).
- **NFR-012-02** Subprocesso CLI é spawnado com **argv direto (sem shell)**,
  ambiente sanitizado, timeout com kill, sem arquivos temporários contendo o
  prompt. Nunca logar argv/conteúdo (política de redação do T-003).
  No Windows, shims npm conhecidos são resolvidos para o executável nativo
  da instalação; scripts `.cmd`/`.bat` arbitrários são recusados. Configurações
  pessoais, hooks, plugins e MCP não são carregados pelos adaptadores
  estáveis; somente a autenticação existente é reutilizada. Argumentos
  extras aceitam exclusivamente opções verificadas de geração de texto e
  não podem alterar essas proteções. Versões sem os flags exigidos falham
  sem recorrer a um modo menos protegido.
- **NFR-012-03** Saída do provider limitada (ex.: 32k chars); resposta muito
  longa é truncada com aviso, não estoura o painel.
- **NFR-012-04** O painel segue a direção visual "Papel & Anil" ([ADR-0003](../../../docs/adr/0003-identidade-visual-papel-e-anil.md)) e
  os mesmos tokens da Flow Bar.

## Critérios de aceitação

- **AC-012-01** _Dado_ `codex` instalado e logado, _quando_ ativo o provider
  `cli_agent/codex` e gero um resumo de reunião, _então_ o resumo é produzido
  pela assinatura, sem API key configurada.
- **AC-012-02** _Dado_ nenhum CLI no PATH, _quando_ abro Modelos & Provedores,
  _então_ os providers CLI aparecem como `ausente`, desabilitados.
- **AC-012-03** _Dado_ o painel aberto pelo atalho (já ditando), _quando_
  falo uma pergunta e aperto o atalho de novo, _então_ a transcrição é
  enviada, a resposta aparece no painel e uma pergunta seguinte mantém o
  contexto da conversa.
- **AC-012-04** _Dado_ o painel arrastado e fixado num canto, _quando_ reinicio
  o app, _então_ ele reabre na mesma posição.
- **AC-012-05** _Dado_ uma chamada em andamento, _quando_ aperto `Esc` ou o
  cancelar do painel, _então_ o subprocesso é encerrado e nenhuma resposta
  parcial fica presa.
- **AC-012-06** _Dado_ sem provider, _quando_ abro o assistente, _então_ vejo
  o aviso e o ditado não é transcrito para envio.

## Casos de borda

- CLI que responde pedindo confirmação interativa → o adaptador deve usar os
  flags non-interactive; se o binário não suportar headless, o adaptador fica
  desabilitado com log claro.
- CLI demorando demais → timeout configurável, cancelável, sem travar o painel.
- Painel fixado em monitor desconectado → reposiciona no primário.
- Ditado enquanto uma resposta está chegando → o prompt é enfileirado e
  enviado quando a resposta comita; cancelar o turno descarta a fila
  junto (cancelar é cancelar).
- Painel fechado durante um ditado roteado → a claim é descartada e o texto
  **não** é colado no app focado nem enviado — fechar no meio do ditado é
  a forma de abortar sem enviar.
- Prompt/resposta com conteúdo sensível → mesma política de logs do T-003;
  retenção segue F011 (o painel não persiste conversa por padrão).
