# F009 — Notetaker de reuniões

**Status**: Draft · **Release**: v1 · **Depende de**: F003, F008, LlmProvider (T-050, BYOK) — o editor de "Minhas notas" é um Markdown embutido na janela da reunião; o Scratchpad completo (F007) é v1.1+

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

- **FR-009-01** Formas de iniciar: toast (F008), 2º botão da Flow Bar, atalho `Alt+M` (alterna), Hub → "Nova reunião" (opções: _Chamada no computador_ = mic + sistema; _Presencial_ = só mic).
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
- **FR-009-10** **Ditado durante a reunião**: o engine de áudio faz _fan-out_ do mic; enquanto houver sessão de ditado, os trechos da trilha `mic` no intervalo são marcados e **excluídos** da transcrição, e um `dictation_marker` ("Ditado") é inserido. A trilha `system` continua normal.
- **FR-009-11** Eco (usuário sem fone, alto-falante vaza no mic): P1 — deduplicação: se um segmento `mic` coincide no tempo com fala em `system` e similaridade textual ≥ 0,8, descartar o segmento `mic`. P2 — cancelamento de eco (AEC, WebRTC APM) usando `system` como referência.

### Janela da reunião

- **FR-009-12** Cabeçalho: título editável (padrão "<App> · <data hora>"), app, cronômetro, status, Pausar/Parar.
- **FR-009-13** Abas:
  - **Minhas notas** — editor Markdown com autosave; é o foco padrão. Na v1 é um editor embutido nesta janela (o Scratchpad da F007 é v1.1+).
  - **Transcrição** — segmentos com horário e falante: `Você` (trilha mic), `Outros` ou `Falante N` (trilha system, após diarização), marcadores de ditado/lacuna.
  - **Resumo** — disponível após o processamento; editável.
- **FR-009-14** Fechar a janela **não** para a gravação; a Flow Bar/bandeja reabre a janela.
  Ao reutilizar a janela para outra reunião, transcrição, notas, resumo, estado
  e cronômetro pertencem exclusivamente ao novo ID. Eventos e consultas
  atrasados da reunião anterior não podem contaminar a nova. A hidratação
  da mesma reunião continua preservando os segmentos recebidos ao vivo.
- **FR-009-15** Transcrição durante a reunião (configurável, padrão ligada): cada bloco selado de 60 s por trilha é enviado **inteiro, numa única chamada** ao provedor de reuniões assim que sela — o atraso é a cadência do bloco (~60 s), não um alvo de latência. O VAD apara só o silêncio das pontas (450 ms de folga de cada lado) e descarta blocos sem fala; pausas internas permanecem na chamada. Com a opção desligada, transcreve só ao final, pelo mesmo caminho. Na v1 o provedor é sempre local; se o modelo estiver ocupado/indisponível, os blocos entram em fila.
  - Fatiar o bloco por fala foi medido como pior (15% das falas sem texto, alucinações nas bordas dos cortes, frases quebradas) e mais lento — ver [ADR-0005](../../../docs/adr/0005-transcricao-de-reuniao-por-bloco.md). O preço aceito: as linhas do transcript têm granularidade de bloco, não de frase.
- **FR-009-16** O texto de cada segmento de reunião passa pelo mesmo pipeline determinístico do ditado (normalizar → dicionário → limpeza `light`), **sem** a etapa de comandos de voz: numa reunião "enviar" ou "nova linha" são fala, não comando.

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
- **FR-009-21** Sem chave de LLM configurada (BYOK, F011/T-016) → a reunião fica `ready` só com transcrição e notas, com aviso para configurar o resumo. Transcrição e notas nunca dependem do LLM.

Em Configurações → Resumos de reunião, o usuário pode testar o provedor e o modelo selecionados com uma mensagem sintética curta. O teste não envia conteúdo de reuniões, respeita o modo offline e informa sucesso ou uma falha classificada (chave, rede, cota, tempo limite ou configuração). O botão fica indisponível durante a gravação das configurações e o resultado é descartado quando elas mudam. A interface informa que a chamada pode consumir uso da API. Listar modelos disponíveis não constitui sucesso desse teste.

- **FR-009-22** Falha no processamento → `error` com "Tentar novamente"; áudio preservado.
- **FR-009-27** Avisos (`toast://show`) se auto-dispensam após 30 s com uma barra de contagem regressiva visível; hover pausa a contagem. Avisos que aguardam resposta (check-in FR-009-09) são isentos.

### Exportação e busca

- **FR-009-23** "Copiar como Markdown": título, data/hora, duração, app, Minhas notas, Resumo, Transcrição (`[mm:ss] Falante: texto`).
- **FR-009-24** P2 — salvar `.md` numa pasta de exportação (o Wispr só tem "Copy to Markdown", FR-009-23).
- **FR-009-25** Lista de reuniões no Hub com busca full-text em título, notas, resumo e transcrição.
- **FR-009-28** Cada item da lista (`meeting_list`/`meeting_search`) e o detalhe (`meeting_get`) trazem, além da linha da reunião: `source_app { exe, name }` (app de origem — `null` em reunião manual/presencial) e `list_status` (`recording` | `paused` | `processing` | `no_summary` | `failed` | `ready`, derivado de `status` × `summary_status`, ver [data-model §2](../../architecture/data-model.md)). O ícone do app vem de `meeting_source_icon(id)` — data URI PNG extraído do executável salvo (`meetings.app_exe_path`), em cache por caminho, ou `null` (app conhecido/navegador, reunião sem caminho, falha ou timeout de 2 s) → a UI mostra o logo embutido ou um monograma. O caminho do `.exe` nunca sai por IPC. Retentar uma linha `failed`: `meeting_retry_processing` quando `meeting.status` é `error`/`recovered`; `meeting_regenerate_summary` quando é `ready` (resumo falhou).
- **FR-009-29** Notetaker no Hub (design Papel & Anil): cabeçalho com "Iniciar Notetaker" (menu Chamada no computador / Presencial); bloco **Agora** (só durante gravação/pausa: título, cronômetro vindo de `meeting://state`, pausar/retomar, parar) e linha de estado da detecção (ativa / pausada / desligada, derivada de `meeting_detection_enabled`, `meeting_detection_paused_until_ms` e `offline_mode`); abas "Notas passadas" e "Transcrevendo N" (a aba só existe enquanto há reunião processando; o percentual vem de `meeting://progress`); lista agrupada por dia (Hoje, Ontem, data por extenso) com o logo do app de origem e **um** chip de estado (Resumo pronto, Transcrevendo %, Sem resumo → leva a Configurações, Falhou + "Tentar novamente"). Logo: embutido para apps conhecidos (Zoom, Teams, Meet, Webex, Discord, Slack; tenta o nome amigável e depois o exe), senão `meeting_source_icon` chamado de forma preguiçosa (só linhas já visíveis) e em cache por exe, senão monograma; reunião manual/presencial usa a marca do app. Clicar na linha abre o painel de detalhes (resumo por seções, trecho da transcrição, copiar resumo/Markdown, renomear, excluir com confirmação, abrir transcrição completa na janela da reunião); Esc fecha. Em painéis estreitos (< ~760 px) o detalhe flutua sobre a lista. Sem calendário (P2). Limitações v1: "Pausar detecção por 1 h" só existe na bandeja (não há comando IPC; a linha de detecção aponta para lá); ícones de navegadores sem logo embutido caem no monograma.
- **FR-009-26** P2: exportar áudio (mix) e legendas SRT.

## Requisitos não funcionais

- **NFR-009-01** Uso de CPU durante gravação sem transcrição ao vivo ≤ 3 %.
- **NFR-009-02** Reunião de 30 min pronta (transcrição local + resumo) ≤ 2 min com LLM em nuvem (BYOK); ≤ 6 min com LLM local.
- **NFR-009-03** Perda máxima de áudio em caso de crash: 60 s.
- **NFR-009-04** Disco: ~1,9 MB/min por trilha (WAV 16 kHz mono 16-bit); P1: FLAC (~50 %).

## Critérios de aceitação

- **AC-009-01** _Dado_ uma chamada de teste no Meet com outra pessoa falando, _quando_ gravo 5 min e paro, _então_ a transcrição tem falas de `Você` e de `Outros` nas posições corretas e o resumo lista as decisões combinadas.
- **AC-009-02** _Quando_ fecho a janela da reunião, _então_ a gravação continua (cronômetro na Flow Bar) e reabrir mostra tudo.
- **AC-009-03** _Dado_ uma gravação ativa, _quando_ dito uma mensagem no Slack com `Ctrl+Win`, _então_ o texto vai para o Slack, **não** aparece na transcrição e há um marcador "Ditado" no horário. O bloco de `mic` que contém o ditado é dividido nas fronteiras do intervalo: só o trecho ditado fica excluído, e a fala de reunião em volta dele continua na ata (ADR-0005). A trilha `system` nunca é excluída.
- **AC-009-04** _Quando_ mato o processo durante uma gravação e reabro o app, _então_ a reunião aparece como recuperada e, ao processar, contém tudo até ≤ 60 s antes do crash.
- **AC-009-05** _Quando_ troco do alto-falante para um fone Bluetooth durante a gravação, _então_ o áudio dos outros continua sendo capturado.
- **AC-009-06** _Dado_ "Minhas notas" com "cliente pediu desconto", _então_ o resumo considera essa informação e o texto de "Minhas notas" permanece idêntico após gerar o resumo.
- **AC-009-07** _Quando_ clico em "Copiar como Markdown" e colo no Notion, _então_ os títulos, listas e checkboxes aparecem formatados.
- **AC-009-08** _Dado_ 10 min de silêncio, _então_ aparece o check-in; sem resposta por 2 min, a gravação para.

## Casos de borda

- Loopback sem som (nenhum áudio tocando) é normal — sem erro; porém se o app de reunião usar um dispositivo de saída **não padrão**, avisar: "Os outros participantes não estão sendo capturados" (P1: permitir escolher o dispositivo de saída capturado).
- Microfone em uso exclusivo por outro app → trilha `mic` falha: continuar só com `system` e avisar.
- Reunião muito longa (> 4 h) → limite rígido; oferecer iniciar nova.
- Disco cheio → parar com segurança e avisar; os blocos já gravados continuam válidos.

## Notas técnicas

- Loopback: `cpal` no Windows suporta criar _input stream_ sobre um dispositivo de saída (WASAPI loopback); mudanças de dispositivo padrão via `IMMNotificationClient`.
- O `audio engine` tem um único stream de mic com múltiplos assinantes (ditado, reunião, medidor) — nunca abrir o mic duas vezes.
- macOS (v1.0): ScreenCaptureKit / Core Audio process taps (macOS 14.4+), exigindo permissão de gravação de áudio do sistema.

## Expansão especificada — conectores (v1.1+ proposta, não implementada)

[F013: Notetaker e duas fases](../013-connectors/notetaker.md) define primeiro configuração/instalação MCP com OAuth e destinos padrão; depois ações na aba Resumo e detalhe do Hub: vincular/publicar resumo em página Notion e revisar/criar Task, Feature ou item de backlog Azure com seleção de projeto/equipe/backlog/pai/sprint. Vínculos persistem por reunião/versão e regenerar não publica automaticamente. Refs: FR-013-24..31, AC-013-21..28; T-094..T-103. Não altera a aceitação já registrada da v1.
