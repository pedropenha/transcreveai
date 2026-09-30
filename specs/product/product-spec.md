# Spec de Produto — Transcreve.ai

## 1. Visão

Escrever na velocidade da fala, em qualquer aplicativo, com privacidade. E nunca mais sair de uma reunião sem notas.

## 2. Problema

- Digitar é ~4× mais lento que falar. O ditado nativo do Windows (Win+H) produz texto cru, sem limpeza, sem vocabulário próprio e com qualidade fraca em pt-BR.
- Ferramentas como o Wispr Flow resolvem isso, mas exigem assinatura, conta e enviam todo o áudio para a nuvem.
- Notas de reunião exigem bots que entram na chamada ou apps separados.

## 3. Personas

| Persona                              | Contexto                                                             | O que valoriza                                                    |
| ------------------------------------ | -------------------------------------------------------------------- | ----------------------------------------------------------------- |
| **Dev/profissional de conhecimento** | Escreve muito (Slack, e-mail, prompts para IA, código) em pt-BR e en | Velocidade, termos técnicos corretos, funcionar em IDE e terminal |
| **Pessoa de muitas reuniões**        | Gestor(a), consultor(a), PM                                          | Resumo e próximos passos automáticos, sem bot na chamada          |
| **Preocupado(a) com privacidade**    | Jurídico, saúde, empresa com política restrita                       | Tudo local, nada de nuvem                                         |

## 4. Objetivos e métricas

| Objetivo       | Métrica                                                       | Meta v1                                            |
| -------------- | ------------------------------------------------------------- | -------------------------------------------------- |
| Ditado rápido  | Tempo entre soltar o atalho e o texto aparecer (fala de 10 s) | p50 ≤ 1,5 s (local GPU)                            |
| Ditado preciso | WER no conjunto de fixtures pt-BR                             | ≤ 8 % com large-v3-turbo                           |
| Sem atrito     | % de sessões inseridas sem erro                               | ≥ 99 %                                             |
| Reuniões úteis | Resumo pronto após parar (reunião de 30 min)                  | ≤ 2 min (LLM em nuvem, BYOK) · ≤ 6 min (LLM local) |
| Leve           | RAM ociosa sem modelo carregado                               | ≤ 150 MB                                           |

## 5. Escopo por release

> O fatiamento original (v0.1 → v0.3 → v1.0) foi substituído por um único release **v1** — ver [ADR-0002](../../docs/adr/0002-escopo-v1.md).

### v1 — Ditado + Reuniões

- Flow Bar (idle, hover com 2 ações, gravando, processando, erro, cronômetro de reunião) — [F001](../features/001-flow-bar/spec.md)
- Atalhos globais: push-to-talk, mãos livres, cancelar, colar último, Notetaker — [F002](../features/002-hotkeys-dictation/spec.md)
- Transcrição **somente local** (whisper.cpp); modelo escolhido no onboarding com `large-v3-turbo` como recomendação — [F003](../features/003-transcription-engines/spec.md)
- Pipeline determinístico: dicionário (dicas de vocabulário), comandos de voz "nova linha"/"enviar", filtro de alucinação, limpeza `light` de muletas pt-BR — [F004](../features/004-text-pipeline/spec.md)
- Inserção `auto` via área de transferência com restauração; janela elevada → `clipboard_only` + aviso — [F005](../features/005-text-insertion/spec.md)
- Detecção de reunião + toast (Zoom, Teams, Meet, Webex) — [F008](../features/008-meeting-detection/spec.md)
- Notetaker: mic + áudio do sistema, transcrição, resumo **via LLM BYOK** (sem chave → sem resumo), exportação — [F009](../features/009-meeting-notetaker/spec.md)
- Hub completo: histórico, reuniões, dicionário, modelos, configurações; onboarding; bandeja — [F010](../features/010-hub-settings/spec.md)
- Segredos no cofre do SO (chave LLM inclusa) — [F011](../features/011-security-privacy/spec.md)
- Release: `bun run tauri build` local (NSIS, sem assinatura/updater).

### v1.1+ — Texto inteligente e nuvem

- STT em nuvem (OpenAI, Groq, compatível OpenAI) com chave própria — F003, T-014
- Pipeline completo: limpeza por LLM com níveis, backtrack, estilos por app, snippets — [F004](../features/004-text-pipeline/spec.md)
- Command Mode — [F006](../features/006-command-mode/spec.md)
- Scratchpad / notas por voz — [F007](../features/007-notes-scratchpad/spec.md)

### v1.1+ — Polimento e portes

- Parakeet local, diarização, builds com GPU dedicada, assinatura de código, auto-update, macOS e Linux, acessibilidade completa.

### Fora de escopo (v1)

- Qualquer funcionalidade que o Wispr Flow não tenha, exceto a transcrição local / com chave própria (constituição, princípios I e II).
- Contas, sincronização entre dispositivos, servidor próprio.
- Apps móveis.
- Integração com calendário (P2).
- Bot que entra na chamada (**nunca** — captura é sempre local).
- Tradução automática (P2, opt-in).

## 6. Jornadas principais

**J1 — Ditar uma mensagem no Slack**

1. Cursor na caixa de mensagem. Usuário segura `Ctrl+Win`.
2. Flow Bar expande com ondas de áudio; som curto de início.
3. Fala: "oi pessoal, é, o deploy vai ficar pra amanhã às 3, não, na verdade às 4, enviar".
4. Solta o atalho. Flow Bar mostra "processando".
5. Em ~1 s aparece "Oi pessoal, o deploy vai ficar para amanhã às 4." e o Enter é pressionado.

**J2 — Ditado longo mãos-livres**: toque duplo em `Ctrl+Win` → fala por 3 min → toque para parar → texto inserido em parágrafos.

**J3 — Reescrever texto selecionado (Command Mode)**: seleciona um parágrafo no Gmail → segura `Ctrl+Win+Alt` → "deixa mais formal e mais curto" → o parágrafo é substituído.

**J4 — Nota rápida por voz**: `Ctrl+Win+Shift` → "lembrar de revisar o contrato da Acme na sexta" → toast "Nota salva" → aparece no Scratchpad.

**J5 — Reunião no Google Meet**

1. Usuário entra na chamada no Chrome. Em ~5 s aparece o toast "Reunião detectada · Agora".
2. Hover → "Iniciar Notetaker". Clique. A Flow Bar mostra um ponto vermelho com cronômetro.
3. Durante a reunião, digita observações em "Minhas notas" e dita uma resposta no chat do Meet (o ditado não entra na transcrição).
4. Sai da chamada → gravação para sozinha após 15 s → "Gerando notas…" → resumo com decisões e próximos passos.
5. "Copiar como Markdown" → cola no Notion.

**J6 — Configurar pela primeira vez**: onboarding → permissão de mic → escolher o modelo local (com `large-v3-turbo` marcado como recomendado; baixa com progresso) → treina o atalho num campo de teste.

## 7. Glossário

| Termo                  | Definição                                                                        |
| ---------------------- | -------------------------------------------------------------------------------- |
| **Flow Bar**           | Janela flutuante mínima, sempre visível, ponto de entrada visual.                |
| **Hub**                | Janela principal do app (histórico, notas, reuniões, configurações).             |
| **Sessão de ditado**   | Ciclo gravar → transcrever → processar → inserir, iniciado por atalho ou clique. |
| **Push-to-talk (PTT)** | Grava enquanto o atalho estiver pressionado.                                     |
| **Mãos livres**        | Gravação alternada (liga/desliga) sem segurar teclas.                            |
| **Command Mode**       | Instrução por voz que transforma o texto selecionado ou gera texto novo.         |
| **STT / ASR**          | Speech-to-text; o motor de transcrição.                                          |
| **Provedor**           | Implementação concreta de STT ou LLM (local ou nuvem).                           |
| **Pipeline de texto**  | Etapas determinísticas + LLM que transformam o texto cru no texto final.         |
| **Perfil de app**      | Regras por aplicativo (estilo, nível de limpeza, método de inserção).            |
| **Notetaker**          | Sessão de gravação de reunião com transcrição e resumo.                          |
| **Trilha**             | Fonte de áudio de uma reunião: `mic` (você) ou `system` (outros participantes).  |
| **VAD**                | Voice Activity Detection; detecta trechos com fala.                              |
