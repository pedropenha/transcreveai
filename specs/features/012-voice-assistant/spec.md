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
- Overlay do assistente: aberto por atalho global configurável ou pela Flow
  Bar; ditado transcrito ao vivo; envio por tecla/comando; resposta no painel;
  multi-turn dentro da sessão.
- Painel **arrastável e fixável** — independente da posição da Flow Bar.

**Fora (não fazer agora):**

- Janela de chat completa no Hub / histórico persistente de conversas.
- Anexar contexto de reunião ao assistente (P2).
- Command Mode (F006) continua separado — lá a resposta é inserida como texto;
  aqui a resposta fica no overlay e o usuário decide o que fazer com ela.

## Histórias

- **US-012-01** Quero apertar um atalho em qualquer app, ditar "que horas é a
  reunião de amanhã no meu calendário" e ler a resposta num painel.
- **US-012-02** Quero usar meu Codex/Claude Code já logado, sem configurar
  API key.
- **US-012-03** Quero ditar, revisar o texto transcrito e só depois enviar.
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
  Alvos iniciais: `codex exec`, `claude -p`, `devin` (modo headless),
  `cursor-agent -p`. Os flags exatos são confirmados na implementação contra a
  versão instalada.
- **FR-012-02** Detecção: o provider aparece na tela Modelos & Provedores como
  `detectado`/`ausente` conforme o binário estar no `PATH`. Ausente → linha
  desabilitada com dica de instalação.
- **FR-012-03** **Sem chave de API**: a autenticação é a sessão do próprio CLI
  (assinatura). Nada vai para o keyring.
- **FR-012-04** O roteador cost-aware (T-050) trata providers CLI como
  **custo zero**; o usuário escolhe o provider do resumo e do assistente nas
  configurações (default: primeiro provider configurado/detectado).
- **FR-012-05** Por provider: habilitar/desabilitar, caminho do binário
  (override), args extras e timeout (padrão 60 s para assistente, 180 s para
  resumo map-reduce).

### Overlay do assistente

- **FR-012-10** Binding `assistant`: atalho global configurável (mesma tela de
  captura dos demais atalhos, F010) **e** botão na Flow Bar (terceiro slot,
  ao lado de Ditar/Notetaker).
- **FR-012-11** O atalho/botão abre um painel overlay `topmost` que **não
  rouba foco** ao surgir; o foco vai ao painel só quando o usuário clicar/ditar.
- **FR-012-12** Com o painel aberto, falar inicia a transcrição ao vivo no
  campo de entrada — mesmo pipeline de STT do ditado, sem limpeza LLM. O texto
  **nunca é inserido em outros apps**; o destino é o painel.
- **FR-012-13** Envio: `Enter` com o painel focado, botão "Enviar" ou uma
  segunda pressão do atalho `assistant`. O campo é editável antes do envio
  (teclado comum funciona no painel).
- **FR-012-14** Resposta renderizada no painel (Markdown leve: negrito,
  listas, código inline). Durante a chamada: estado `thinking` com botão de
  cancelar; erro do provider → estado de erro com `Tentar de novo`.
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

## Requisitos não-funcionais

- **NFR-012-01** Overlay abre em < 200 ms; transcrição ao vivo com a mesma
  latência do ditado; painel nunca bloqueia a sessão de ditado em andamento
  (coexistência como F009/FR-009-15).
- **NFR-012-02** Subprocesso CLI é spawnado com **argv direto (sem shell)**,
  ambiente sanitizado, timeout com kill, sem arquivos temporários contendo o
  prompt. Nunca logar argv/conteúdo (política de redação do T-003).
- **NFR-012-03** Saída do provider limitada (ex.: 32k chars); resposta muito
  longa é truncada com aviso, não estoura o painel.
- **NFR-012-04** O painel segue a direção visual "Sinal Calmo" (T-008) e
  os mesmos tokens da Flow Bar.

## Critérios de aceitação

- **AC-012-01** _Dado_ `codex` instalado e logado, _quando_ ativo o provider
  `cli_agent/codex` e gero um resumo de reunião, _então_ o resumo é produzido
  pela assinatura, sem API key configurada.
- **AC-012-02** _Dado_ nenhum CLI no PATH, _quando_ abro Modelos & Provedores,
  _então_ os providers CLI aparecem como `ausente`, desabilitados.
- **AC-012-03** _Dado_ o painel aberto pelo atalho, _quando_ dito uma pergunta
  e aperto Enter, _então_ a resposta aparece no painel e uma pergunta seguinte
  mantém o contexto da conversa.
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
- Ditado enquanto uma resposta está chegando → enfileirar no campo de entrada,
  não descartar.
- Prompt/resposta com conteúdo sensível → mesma política de logs do T-003;
  retenção segue F011 (o painel não persiste conversa por padrão).
