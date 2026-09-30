# Spec de Produto — Sussurro

## 1. Visão

Escrever na velocidade da fala, em qualquer aplicativo, com privacidade. E nunca mais sair de uma reunião sem notas.

## 2. Problema

- Digitar é ~4× mais lento que falar. O ditado nativo do Windows (Win+H) produz texto cru, sem limpeza, sem vocabulário próprio e com qualidade fraca em pt-BR.
- Ferramentas como o Wispr Flow resolvem isso, mas exigem assinatura, conta e enviam todo o áudio para a nuvem.
- Notas de reunião exigem bots que entram na chamada ou apps separados.

## 3. Personas

| Persona | Contexto | O que valoriza |
|---|---|---|
| **Dev/profissional de conhecimento** | Escreve muito (Slack, e-mail, prompts para IA, código) em pt-BR e en | Velocidade, termos técnicos corretos, funcionar em IDE e terminal |
| **Pessoa de muitas reuniões** | Gestor(a), consultor(a), PM | Resumo e próximos passos automáticos, sem bot na chamada |
| **Preocupado(a) com privacidade** | Jurídico, saúde, empresa com política restrita | Tudo local, nada de nuvem |

## 4. Objetivos e métricas

| Objetivo | Métrica | Meta v1 |
|---|---|---|
| Ditado rápido | Tempo entre soltar o atalho e o texto aparecer (fala de 10 s) | p50 ≤ 1,0 s (nuvem) · ≤ 1,5 s (local GPU / Parakeet CPU) |
| Ditado preciso | WER no conjunto de fixtures pt-BR | ≤ 8 % com large-v3-turbo |
| Sem atrito | % de sessões inseridas sem erro | ≥ 99 % |
| Reuniões úteis | Resumo pronto após parar (reunião de 30 min) | ≤ 2 min (nuvem) · ≤ 6 min (local) |
| Leve | RAM ociosa sem modelo carregado | ≤ 150 MB |

## 5. Escopo por release

### MVP (v0.1) — Ditado
- Flow Bar (idle, hover com 2 ações, gravando, processando, erro) — [F001](../features/001-flow-bar/spec.md)
- Atalhos globais: push-to-talk, mãos livres, cancelar, colar último — [F002](../features/002-hotkeys-dictation/spec.md)
- Transcrição local (whisper.cpp) + nuvem (OpenAI, Groq, compatível OpenAI) com chave própria — [F003](../features/003-transcription-engines/spec.md)
- Inserção via área de transferência com restauração — [F005](../features/005-text-insertion/spec.md)
- Hub com histórico e configurações; onboarding; bandeja — [F010](../features/010-hub-settings/spec.md)
- Segredos no cofre do SO — [F011](../features/011-security-privacy/spec.md)
- Pipeline mínimo: dicionário (dicas de vocabulário), comandos de voz "nova linha"/"enviar", filtro de alucinação.

### v0.2 — Texto inteligente
- Pipeline completo: limpeza por LLM com níveis, backtrack, estilos por app, snippets — [F004](../features/004-text-pipeline/spec.md)
- Command Mode — [F006](../features/006-command-mode/spec.md)
- Scratchpad / notas por voz — [F007](../features/007-notes-scratchpad/spec.md)

### v0.3 — Reuniões
- Detecção de reunião + toast — [F008](../features/008-meeting-detection/spec.md)
- Notetaker: mic + áudio do sistema, transcrição, resumo, exportação — [F009](../features/009-meeting-notetaker/spec.md)

### v1.0 — Polimento
- Parakeet local, diarização, builds com GPU, assinatura de código, auto-update, macOS, acessibilidade completa.

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

**J6 — Configurar pela primeira vez**: onboarding → permissão de mic → escolher "Local" (baixa o modelo recomendado para o hardware) ou "Nuvem" (cola a chave, testa) → treina o atalho num campo de teste.

## 7. Glossário

| Termo | Definição |
|---|---|
| **Flow Bar** | Janela flutuante mínima, sempre visível, ponto de entrada visual. |
| **Hub** | Janela principal do app (histórico, notas, reuniões, configurações). |
| **Sessão de ditado** | Ciclo gravar → transcrever → processar → inserir, iniciado por atalho ou clique. |
| **Push-to-talk (PTT)** | Grava enquanto o atalho estiver pressionado. |
| **Mãos livres** | Gravação alternada (liga/desliga) sem segurar teclas. |
| **Command Mode** | Instrução por voz que transforma o texto selecionado ou gera texto novo. |
| **STT / ASR** | Speech-to-text; o motor de transcrição. |
| **Provedor** | Implementação concreta de STT ou LLM (local ou nuvem). |
| **Pipeline de texto** | Etapas determinísticas + LLM que transformam o texto cru no texto final. |
| **Perfil de app** | Regras por aplicativo (estilo, nível de limpeza, método de inserção). |
| **Notetaker** | Sessão de gravação de reunião com transcrição e resumo. |
| **Trilha** | Fonte de áudio de uma reunião: `mic` (você) ou `system` (outros participantes). |
| **VAD** | Voice Activity Detection; detecta trechos com fala. |
