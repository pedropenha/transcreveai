# Pesquisa: como o Wispr Flow funciona

> Levantamento feito em 2026-09-30 a partir da documentação oficial (docs.wisprflow.ai), reviews e dos prints fornecidos. Serve de referência de comportamento, **não** de cópia de marca ou visual.

## 1. Visão geral

O Wispr Flow é um "teclado por voz" que funciona em qualquer aplicativo. O usuário aperta um atalho, fala naturalmente, e o texto já limpo e formatado aparece onde o cursor está (Gmail, Slack, VS Code, qualquer formulário). O processamento é feito na **nuvem**: modelos de ASR + LLM removem vícios de linguagem, corrigem a estrutura e formatam o texto de acordo com o app ativo.

No desktop ele vive em dois lugares:

- **Hub** — janela principal: histórico, personalização (dicionário, snippets, estilos), configurações, notas e reuniões.
- **Flow Bar** — controle flutuante na parte inferior da tela, onde acontece o uso do dia a dia.
- Ícone na **bandeja do Windows** / barra de menus do Mac.

## 2. Flow Bar

- Barra pequena e fixa na parte inferior da tela, sempre visível sobre outros apps.
- Clicar na bolha central inicia o ditado; clique direito abre opções rápidas (inclusive "soneca" para esconder durante filmes/apresentações).
- Pode ser arrastada e "encaixada" nas bordas esquerda/direita; a posição é lembrada.
- Pode ser ocultada nas configurações (Hub → Settings → System).

### O que os prints mostram

| # | Estado | Observação |
|---|---|---|
| 1 | **Idle** | Pílula mínima (~50×10 px), fundo escuro, borda clara, centralizada acima da barra de tarefas. |
| 2 | **Hover** | Expande para uma cápsula com **dois botões circulares**: 🎤 *Ditar* e ◉ *Notetaker/gravar* (com um ponto verde de status). Tooltip mostra o atalho: "Dictate **Win + Space**". |
| 3 | **Reunião detectada (compacto)** | Toast escuro: ícone de câmera amarelo, "Meeting detected", ponto verde + "Now". |
| 4 | **Reunião detectada (hover)** | O toast expande e ganha um **X** (dispensar), o botão **"Start Notetaker"** (ícone de ondas sonoras) e um **chevron ▾** com mais opções. |

## 3. Atalhos e modos de ditado

- **Push-to-talk**: segurar o atalho (padrão Fn no Mac; no Windows, Ctrl+Win), falar, soltar.
- **Mãos livres**: toque duplo no atalho ou atalho dedicado (ex.: Fn+Space) alterna gravação contínua; toque de novo para parar.
- Sessões de ditado de até **20 minutos**.
- Dizer **"press enter"** no fim insere o texto e aperta Enter (útil em chats).
- Mais de 100 idiomas, com detecção automática inclusive com troca no meio da frase.

## 4. Inteligência de texto

| Recurso | Comportamento |
|---|---|
| **Auto Cleanup** | Controla o quanto o Flow edita o ditado (níveis). Remove "é", "tipo", "ahn", corrige pontuação. |
| **Backtrack** | Entende correções faladas: "às 3, não, na verdade às 4" → "às 4". |
| **Smart Formatting** | Listas, números, datas, e-mails formatados automaticamente. |
| **Styles** | Tom por categoria de app: Formal, Casual, Muito casual (ex.: mensagens pessoais × e-mail de trabalho). |
| **Dictionary** | Palavras próprias (nomes, jargões) que o usuário ensina; também aprende correções. |
| **Snippets** | Frase-gatilho falada → texto longo (ex.: "meu link de agenda" → URL). |
| **Command Mode** | Selecionar texto e dizer "deixa mais amigável", "transforma em lista"; o texto é reescrito no lugar. |
| **Transforms** | Prompts personalizados salvos para reescrever texto. |
| **Scratchpad** | Área de notas dentro do app para guardar e editar ditados. |
| Integrações | IDEs (Cursor, VS Code), terminais, desktop remoto (Citrix/RDP), Outlook; servidor MCP remoto. |

## 5. Notetaker (reuniões)

- **Detecção**: com "Automatically detect any call" ligado, ao entrar numa chamada aparece um prompt para começar a tomar notas. Detecta Zoom, Microsoft Teams e reuniões no navegador (Google Meet no Chrome etc.), acompanhando a aba mesmo se ela mudar de título.
- **Prompt**: "Transcribe this meeting?" → *Yes* / *Another time*. A gravação só começa após confirmar, a não ser que **"Start Notetaker automatically"** esteja ligado (depende da detecção estar ligada).
- **Início manual**: pelo calendário ("Join and record"), atalho **Alt+M** (Windows) / Option+M (Mac), ou nova nota para reunião presencial.
- **Captura**: microfone **+ áudio do sistema** (no Mac exige permissão de gravação de tela/áudio do sistema).
- **Durante a reunião**: "pílula" de gravação sempre visível com botão Stop; fechar a janela **não** para a gravação; transcrição ao vivo opcional; área para notas próprias.
- **Limites**: duração máxima configurável (30 min / 1 h / 2 h / 3 h; padrão 2 h); **auto-stop** quando a chamada termina; detecção de silêncio e check-ins periódicos ("ainda está em reunião?").
- **Ditar durante reunião**: o microfone é compartilhado por turnos — ao ditar, o Notetaker cede o mic; as palavras ditadas **não** entram na transcrição da reunião e ficam marcadas com um marcador "Dictated".
- **Depois**: processamento de 2–5 min; abas **My thoughts** (notas do usuário, nunca reescritas), **Summary** (resumo editável com próximos passos e responsáveis; tarefas de grupo como "(Everyone)") e **Transcript** (com nomes de falantes quando identificados, senão "Speaker 1/2").
- **Exportação**: "Copy to Markdown" com detalhes, participantes, notas, resumo, chat e transcrição.

## 6. Alternativas open-source (referências técnicas)

| Projeto | Stack | O que aproveitar |
|---|---|---|
| **Handy** (MIT) | Rust + Tauri | Arquitetura quase idêntica à que queremos: whisper.cpp + Parakeet (ggml/ONNX), Silero VAD, overlay always-on-top, atalho segurar/alternar, colagem via clipboard + teclas. |
| **OpenWhispr** | Electron | "Alternativa open-source ao Wispr Flow e Granola": Whisper e Parakeet locais, notas de reunião. |
| **VoiceInk** (GPL) | Swift (Mac) | Referência de UX no macOS. |
| **whisrs** | Rust (Linux) | Vários backends (Groq, Deepgram, OpenAI REST/Realtime, whisper.cpp) atrás de uma interface — bom modelo para o nosso `SttProvider`. |

## 7. O que vamos copiar × diferenciar

**Copiar (comportamento)**: Flow Bar mínima com hover de 2 ações; push-to-talk + mãos livres; inserção no cursor; limpeza com níveis; estilos por app; dicionário; snippets; Command Mode; Scratchpad; detecção de reunião com toast; Notetaker com abas notas/resumo/transcrição; ditado durante reunião sem poluir a transcrição.

**Único desvio intencional** (constituição, princípio II): transcrição **local** ou com **chave de API própria**, em vez de só nuvem por assinatura. Decorrências diretas: download de modelos, modo offline, provedor por uso (ditado × reunião × LLM) com fallback, e nenhuma conta/servidor.

**Detalhe de implementação** (não é funcionalidade nova): falantes "Você × Outros" vêm de duas trilhas (mic × áudio do sistema), aproximando os rótulos de falante do Wispr.

Qualquer outra funcionalidade que o Wispr não tenha fica fora de escopo ou em P2.

## Fontes

- [Navigating the Wispr Flow App](https://docs.wisprflow.ai/articles/5096240724-navigating-the-wispr-flow-app-desktop-ios-and-android)
- [Troubleshooting the Flow Bar](https://docs.wisprflow.ai/articles/5002934560-why-is-the-wispr-bar-is-not-appearing-or-disappearing)
- [Flow bar drags to left or right screen edge (release note)](https://releases.sh/release/rel_ENZi2keTYaYfNSsvVSsAC-flow-bar-drags-to-left-or-right-screen-edge)
- [Using Wispr Flow (coleção de artigos)](https://docs.wisprflow.ai/ml/collections/7303194563-using_wispr_flow)
- [Recording a meeting with Notetaker](https://docs.wisprflow.ai/articles/9238501024-recording-a-meeting-with-notetaker-beta)
- [Notetaker settings, explained](https://docs.wisprflow.ai/articles/9319084321-notetaker-settings-explained-beta)
- [Dictating during a meeting with Notetaker](https://docs.wisprflow.ai/articles/8175153619-dictating-during-a-meeting-with-notetaker-beta)
- [Meeting notes and the editor in Notetaker](https://docs.wisprflow.ai/articles/9406970664-meeting-notes-and-the-editor-in-notetaker-beta)
- [Wispr Flow Notetaker is now on Windows](https://wisprflow.ai/post/notetaker-on-windows)
- [Wispr Flow 101 — Sid Saladi](https://sidsaladi.substack.com/p/wispr-flow-101-the-complete-guide)
- [Wispr Flow overview — eesel.ai](https://eesel.ai/blog/wispr-flow-overview)
- [Handy (GitHub)](https://github.com/cjpais/Handy) · [OpenWhispr](https://feedback.openwhispr.com/compare/wisprflow) · [whisrs](https://docs.rs/crate/whisrs/0.1.11)
- [OpenAI Whisper API pricing 2026 — diyai.io](https://diyai.io/ai-tools/speech-to-text/openai-whisper-api-pricing-2026/)
