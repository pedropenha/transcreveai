# F009 — Notetaker de reuniões

**Status**: Draft · **Release**: v0.3 · **Depende de**: F003, F008, F007 (editor), LlmProvider

## Contexto

Gravar o microfone **e** o áudio do sistema (os outros participantes), transcrever, deixar o usuário tomar as próprias notas durante a reunião e, ao final, gerar resumo com decisões e próximos passos. Sem bot na chamada: a captura é local.

## Histórias

- **US-009-01** Quero iniciar as notas pelo toast, pela Flow Bar, por atalho ou manualmente (reunião presencial).
- **US-009-02** Quero ver a transcrição ao vivo e escrever minhas próprias notas ao lado.
- **US-009-03** Quero ditar em outro app durante a reunião sem que isso entre na transcrição.
- **US-009-04** Quero, ao final, um resumo com decisões, próximos passos e responsáveis.
- **US-009-05** Quero copiar tudo como Markdown.
- **US-009-06** Não quero perder a gravação se o app travar.

## Requisitos funcionais

### Início e captura
- **FR-009-01** Formas de iniciar: toast (F008), 2º botão da Flow Bar, atalho `Alt+M` (alterna), Hub → "Nova reunião" (opções: *Chamada no computador* = mic + sistema; *Presencial* = só mic).
- **FR-009-02** Aviso de consentimento: no **primeiro uso**, modal explicando a responsabilidade de informar os participantes (e LGPD); a cada início, lembrete discreto no toast (desativável) com ação "Copiar aviso para o chat" (texto configurável, ex.: "Estou usando um app local para transcrever esta reunião.").
- **FR-009-03** Duas trilhas independentes, alinhadas por relógio monotônico:
  - `mic`: dispositivo de entrada selecionado;
  - `system`: **loopback WASAPI** do dispositivo de saída padrão.
- **FR-009-04** Troca de dispositivo padrão durante a reunião (ex.: conectar fone Bluetooth) → reabrir a captura no novo dispositivo em ≤ 2 s e inserir `gap_marker` na transcrição se houver lacuna.
- **FR-009-05** Gravação em disco incremental em blocos de 60 s por trilha (`audio/meetings/<id>/mic-0001.wav`…), com fsync por bloco. Na inicialização, reuniões em `recording` sem processo ativo são marcadas `recovered` e oferecidas para processamento.
- **FR-009-06** Controles: **Pausar/Retomar** (pausa ambas as trilhas; `gap_marker`), **Parar**.
- **FR-009-07** Indicador de gravação sempre visível enquanto grava (Flow Bar `meeting_recording` + ícone da bandeja vermelho). Não pode ser ocultado.
- **FR-009-08** Duração máxima (30 min / 1 h / 2 h / 3 h / 4 h; padrão 2 h): aviso 5 min antes com "Estender 30 min"; ao atingir, para e processa.
- **FR-009-09** Check-in de silêncio: sem fala em ambas as trilhas por 10 min → toast "Ainda em reunião?" [Continuar] [Parar]; sem resposta em 2 min → para e processa.
- **FR-009-10** **Ditado durante a reunião**: o engine de áudio faz *fan-out* do mic; enquanto houver sessão de ditado, os trechos da trilha `mic` no intervalo são marcados e **excluídos** da transcrição, e um `dictation_marker` ("Ditado") é inserido. A trilha `system` continua normal.
- **FR-009-11** Eco (usuário sem fone, alto-falante vaza no mic): P1 — deduplicação: se um segmento `mic` coincide no tempo com fala em `system` e similaridade textual ≥ 0,8, descartar o segmento `mic`. P2 — cancelamento de eco (AEC, WebRTC APM) usando `system` como referência.

### Janela da reunião
- **FR-009-12** Cabeçalho: título editável (padrão "<App> · <data hora>"), app, cronômetro, status, Pausar/Parar.
- **FR-009-13** Abas:
  - **Minhas notas** — editor Markdown (mesmo da F007), autosave; é o foco padrão.
  - **Transcrição** — segmentos com horário e falante: `Você` (trilha mic), `Outros` ou `Falante N` (trilha system, após diarização), marcadores de ditado/lacuna.
  - **Resumo** — disponível após o processamento; editável.
- **FR-009-14** Fechar a janela **não** para a gravação; a Flow Bar/bandeja reabre a janela.
- **FR-009-15** Transcrição ao vivo (configurável, padrão ligada): cada trilha é segmentada por VAD (blocos ≤ 30 s) e enviada ao provedor de reuniões; atraso alvo ≤ 10 s. Com a opção desligada, transcreve só ao final. Se o provedor for nuvem e estiver offline, os blocos entram em fila.

### Pós-processamento
- **FR-009-16** Ao parar: `processing` com progresso (`meeting://progress`): (1) transcrever blocos pendentes; (2) P1 — refino opcional com modelo de maior qualidade; (3) P1 — diarização da trilha `system`; (4) resumo; (5) título sugerido.
- **FR-009-17** Resumo gerado por LLM com o **template** selecionado; template padrão (pt-BR):
  ```markdown
  ## Resumo
  (3–5 tópicos)
  ## Decisões
  ## Próximos passos
  - [ ] Tarefa — Responsável (ou "Todos") — Prazo (se mencionado)
  ## Pontos em aberto
  ## Tópicos discutidos
  - Tópico (mm:ss)
  ```
  Regras do prompt: não inventar responsáveis nem prazos; tarefas de grupo como "Todos"; usar "Minhas notas" como contexto prioritário; escrever no idioma predominante da reunião; citar horários.
- **FR-009-18** Transcrição maior que o orçamento de contexto do modelo → map-reduce (resumos parciais por blocos de ~20 min e consolidação).
- **FR-009-19** "Minhas notas" **nunca** são reescritas pelo resumo; editar o resumo não altera as notas.
- **FR-009-20** "Regenerar resumo" (ex.: com outro provedor). P2: templates personalizados ("1:1", "Entrevista", "Daily").
- **FR-009-21** Sem LLM configurado → a reunião fica `ready` só com transcrição e notas, com aviso para configurar resumo.
- **FR-009-22** Falha no processamento → `error` com "Tentar novamente"; áudio preservado.

### Exportação e busca
- **FR-009-23** "Copiar como Markdown": título, data/hora, duração, app, Minhas notas, Resumo, Transcrição (`[mm:ss] Falante: texto`).
- **FR-009-24** P2 — salvar `.md` numa pasta de exportação (o Wispr só tem "Copy to Markdown", FR-009-23).
- **FR-009-25** Lista de reuniões no Hub com busca full-text em título, notas, resumo e transcrição.
- **FR-009-26** P2: exportar áudio (mix) e legendas SRT.

## Requisitos não funcionais

- **NFR-009-01** Uso de CPU durante gravação sem transcrição ao vivo ≤ 3 %.
- **NFR-009-02** Reunião de 30 min pronta (transcrição + resumo) ≤ 2 min com nuvem; ≤ 6 min local (GPU).
- **NFR-009-03** Perda máxima de áudio em caso de crash: 60 s.
- **NFR-009-04** Disco: ~1,9 MB/min por trilha (WAV 16 kHz mono 16-bit); P1: FLAC (~50 %).

## Critérios de aceitação

- **AC-009-01** *Dado* uma chamada de teste no Meet com outra pessoa falando, *quando* gravo 5 min e paro, *então* a transcrição tem falas de `Você` e de `Outros` nas posições corretas e o resumo lista as decisões combinadas.
- **AC-009-02** *Quando* fecho a janela da reunião, *então* a gravação continua (cronômetro na Flow Bar) e reabrir mostra tudo.
- **AC-009-03** *Dado* uma gravação ativa, *quando* dito uma mensagem no Slack com `Ctrl+Win`, *então* o texto vai para o Slack, **não** aparece na transcrição e há um marcador "Ditado" no horário.
- **AC-009-04** *Quando* mato o processo durante uma gravação e reabro o app, *então* a reunião aparece como recuperada e, ao processar, contém tudo até ≤ 60 s antes do crash.
- **AC-009-05** *Quando* troco do alto-falante para um fone Bluetooth durante a gravação, *então* o áudio dos outros continua sendo capturado.
- **AC-009-06** *Dado* "Minhas notas" com "cliente pediu desconto", *então* o resumo considera essa informação e o texto de "Minhas notas" permanece idêntico após gerar o resumo.
- **AC-009-07** *Quando* clico em "Copiar como Markdown" e colo no Notion, *então* os títulos, listas e checkboxes aparecem formatados.
- **AC-009-08** *Dado* 10 min de silêncio, *então* aparece o check-in; sem resposta por 2 min, a gravação para.

## Casos de borda

- Loopback sem som (nenhum áudio tocando) é normal — sem erro; porém se o app de reunião usar um dispositivo de saída **não padrão**, avisar: "Os outros participantes não estão sendo capturados" (P1: permitir escolher o dispositivo de saída capturado).
- Microfone em uso exclusivo por outro app → trilha `mic` falha: continuar só com `system` e avisar.
- Reunião muito longa (> 4 h) → limite rígido; oferecer iniciar nova.
- Disco cheio → parar com segurança e avisar; os blocos já gravados continuam válidos.

## Notas técnicas

- Loopback: `cpal` no Windows suporta criar *input stream* sobre um dispositivo de saída (WASAPI loopback); mudanças de dispositivo padrão via `IMMNotificationClient`.
- O `audio engine` tem um único stream de mic com múltiplos assinantes (ditado, reunião, medidor) — nunca abrir o mic duas vezes.
- macOS (v1.0): ScreenCaptureKit / Core Audio process taps (macOS 14.4+), exigindo permissão de gravação de áudio do sistema.
