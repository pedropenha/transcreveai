# F002 — Atalhos globais e sessão de ditado

**Status**: Draft · **Release**: MVP · **Depende de**: F003, F005

## Contexto

O uso principal é pelo teclado: segurar um atalho, falar, soltar. Precisa funcionar em qualquer app, com combinações só de modificadores (ex.: `Ctrl+Win`), o que exige hook de teclado de baixo nível — `RegisterHotKey` não detecta key-up nem combinações só de modificadores.

## Histórias

- **US-002-01** Quero segurar um atalho, falar e soltar para o texto aparecer no cursor (push-to-talk).
- **US-002-02** Quero ditar textos longos sem segurar teclas (mãos livres).
- **US-002-03** Quero cancelar uma gravação sem inserir nada.
- **US-002-04** Quero personalizar os atalhos, inclusive usar `Win+Space` como no Wispr.
- **US-002-05** Quero colar de novo a última transcrição se ela foi para o lugar errado.

## Atalhos padrão

| Ação                                 | Windows (padrão)                                                                | macOS (v1.1+)          | Tipo                           |
| ------------------------------------ | ------------------------------------------------------------------------------- | ---------------------- | ------------------------------ |
| Ditar (push-to-talk)                 | `Ctrl + Win`                                                                    | `Fn`                   | segurar                        |
| Ditar mãos-livres                    | **duplo toque** em `Ctrl + Win` · atalho dedicado opcional (ex.: `Win + Space`) | `Fn + Space`           | alternar                       |
| Command Mode (F006, v1.1+)           | `Ctrl + Win + Alt`                                                              | `Fn + Ctrl`            | segurar                        |
| Nota por voz (F007, v1.1+)           | `Ctrl + Win + Shift`                                                            | `Fn + Shift`           | segurar                        |
| Notetaker (F009)                     | `Alt + M`                                                                       | `Option + M`           | alternar                       |
| Colar última transcrição             | `Alt + Shift + V`                                                               | `Ctrl + Cmd + V`       | pressionar                     |
| Cancelar                             | `Esc` (só durante gravação)                                                     | `Esc`                  | pressionar                     |
| Ditar traduzindo para inglês (T-093) | `Ctrl + Alt + Space`                                                            | `Option + Cmd + Space` | ativação configurada do ditado |

## Requisitos funcionais

### Atalhos

- **FR-002-01** Suportar: (a) só modificadores (`Ctrl+Win`), (b) modificadores + tecla (`Alt+M`), (c) tecla única dedicada (`F13`–`F24`, `Pause`, `ScrollLock`), (d) P2: botões laterais do mouse.
- **FR-002-02** Tela de captura de atalho: o usuário pressiona a combinação desejada; o app mostra a combinação normalizada e valida.
- **FR-002-03** Validação de conflitos: bloquear duplicatas internas; **avisar** (não bloquear) conflitos com atalhos conhecidos do Windows (`Win+Space` troca de idioma, `Win+H` ditado do Windows, `Win+V` histórico da área de transferência, `Ctrl+Win+Setas` áreas de trabalho, `Ctrl+Win+Enter` Narrador, `Alt+Tab`…).
- **FR-002-04** Quando o atalho configurado corresponde, os eventos de tecla que o formam são **suprimidos** para o SO sempre que suprimir for seguro (ex.: `Win+Space` não troca o idioma). Para combinações só de modificadores, não suprimir key-down (não dá para saber a intenção ainda), mas impedir que o `Win` solto abra o Menu Iniciar ("menu mask key": injetar VK `0xE8` antes do key-up do Win).
- **FR-002-05** Combinações que são **prefixo** de outras (`Ctrl+Win` ⊂ `Ctrl+Win+Alt`): a gravação começa no prefixo; se o modificador extra entrar durante a gravação, o **modo** da sessão muda (ditado → comando/nota) sem perder áudio. O modo final é o da maior combinação vista. Na v1 só existe o modo ditado — o mecanismo de promoção entra agora e os modos de comando/nota chegam na v1.1+.
- **FR-002-06** Se, durante `Arming` (< 250 ms), for pressionada uma tecla **não modificadora** que não faça parte de nenhum atalho do app (ex.: `Ctrl+Win+→`), a sessão é cancelada silenciosamente e os eventos seguem para o SO.
- **FR-002-07** Duplo toque: dois toques curtos (< 250 ms cada) com intervalo ≤ 350 ms no atalho de PTT iniciam mãos-livres. Configurável (liga/desliga).
- **FR-002-08** `Esc` só é interceptado enquanto houver gravação; fora disso, passa normalmente.

### Sessão de ditado

- **FR-002-09** Estados: `Idle → Arming → Recording → Transcribing → Processing → Inserting → Done | Error`, conforme diagrama em [plan.md](../../architecture/plan.md#5-máquina-de-estados-da-sessão-de-ditado). Cada transição emite `session://state`.
- **FR-002-10** A captura de áudio inicia em `Arming` (para não cortar a primeira sílaba). P2: "Microfone aquecido" (stream aberto com pré-roll de 500 ms só em memória) se a latência medida não bastar.
- **FR-002-11** Push-to-talk: soltar qualquer tecla da combinação encerra a gravação.
- **FR-002-12** Mãos-livres: encerra por novo toque/duplo toque, clique em ■ na Flow Bar, atalho dedicado ou limite de duração.
- **FR-002-13** Duração máxima configurável (1–20 min, **padrão 5**). Aviso na Flow Bar aos T-60 s; ao atingir, encerra e processa normalmente.
- **FR-002-14** Áudio com < 300 ms **ou** sem fala detectada pelo VAD é descartado sem chamar o provedor; a Flow Bar mostra "Nada ouvido" por 1 s.
- **FR-002-15** No início da sessão, registrar o **contexto alvo**: HWND em primeiro plano, processo (exe), título (título só é persistido/enviado se a privacidade permitir) e perfil de app correspondente (F004, v1.1+).
- **FR-002-16** Se uma nova sessão for iniciada enquanto a anterior ainda processa, ela grava normalmente; as inserções ocorrem **em ordem** (fila FIFO de **N = 5** sessões pendentes, padrão; configurável no Avançado).
- **FR-002-16a** O app em foco é lido no **início** de cada sessão (`TranscribeAction::start`) e movido para o pipeline no `stop` (`dictation_origin`: guardado por binding entre `start` e `stop`, extraído uma única vez), de modo que a sessão seguinte da fila não sobrescreve o app de origem da anterior; é gravado em toda linha de histórico da sessão (F010 FR-010-29).
- **FR-002-17** Comandos de voz de sessão (antes do pipeline de texto): "enviar" / "press enter" / "send" **no final** da fala → aperta Enter após inserir. Lista de frases configurável por idioma.
- **FR-002-18** O áudio de cada sessão fica em memória até a conclusão; se a sessão falhar, é salvo em `audio/dictations` para "Tentar novamente" (respeitando a retenção) — nunca se perde a fala por erro de rede.
- **FR-002-19** "Colar última transcrição" reinsere o `final_text` da sessão mais recente com status inserido/copiado/falhou.
- **FR-002-20** Seleção de microfone: _Padrão do sistema_ (segue mudanças do padrão) ou dispositivo específico; se o específico sumir, usar o padrão e avisar.
- **FR-002-21** Idioma: _Automático_ (detecção pelo motor, restrita à lista de idiomas escolhidos quando o motor suportar) ou fixo.
- **FR-002-22** P2: baixar o volume de mídia (ducking) durante a gravação.
- **FR-002-23** T-093: ação explícita `transcribe_translate` traduz somente a sessão iniciada para inglês, usando um modelo local compatível. O atalho é configurável com as mesmas regras de conflito/ativação do ditado. `translate_to_english`, idioma, modelo e provedores globais não são alterados. A sessão seguinte de ditado comum conserva seu comportamento anterior. É independente do Command Mode F006/T-056; acionamento por voz e versão de lançamento continuam em aberto.
- **FR-002-24** T-093: validar compatibilidade e disponibilidade antes de abrir o microfone e novamente antes de inferir. Ausência, modelo incompatível ou indisponível gera aviso localizado, sem troca automática ou fallback para nuvem. Cancelamento impede colagem; no Windows, uma mudança de janela entre o início e a inserção da tradução interrompe a entrega antes de escrever no clipboard.
- **FR-002-25** T-093: por compartilhar o motor local, rejeitar o início traduzido durante uma gravação de reunião ou carga de modelo em andamento, com aviso localizado e sem abrir o microfone. A entrega verifica destino/cancelamento também após o atraso configurado da colagem e restaura o clipboard se abortada. Scripts externos não são executados pela tradução por não permitirem essa verificação; métodos comuns conservam seu comportamento.

## Requisitos não funcionais

- **NFR-002-01** Callback do hook processa cada evento em < 1 ms (só classifica e envia por canal).
- **NFR-002-02** Tecla pressionada → primeiro sample capturado ≤ 100 ms.
- **NFR-002-03** O hook **não armazena nem transmite** teclas; só compara com as combinações configuradas. Por tratar entrada do usuário, mudanças aqui passam pelo `ecc:security-reviewer` (gatilho de `rules/common/code-review.md`).

## Critérios de aceitação

- **AC-002-01** _Dado_ um campo de texto em foco, _quando_ seguro `Ctrl+Win`, falo "olá mundo" e solto, _então_ "Olá mundo." é inserido e o Menu Iniciar **não** abre.
- **AC-002-02** _Dado_ `Ctrl+Win` segurado por 100 ms e solto sem segundo toque, _então_ nada é inserido e nenhuma requisição ao provedor é feita.
- **AC-002-03** _Quando_ faço duplo toque em `Ctrl+Win`, falo por 2 min e toco de novo, _então_ o texto completo é inserido.
- **AC-002-04** _Dado_ uma gravação ativa, _quando_ aperto `Esc`, _então_ nada é inserido, o áudio é descartado e o `Esc` não chega ao app.
- **AC-002-05** _Dado_ nenhuma gravação, _quando_ aperto `Esc`, _então_ o app em foco recebe o `Esc`.
- **AC-002-06** _Quando_ aperto `Ctrl+Win+→`, _então_ o Windows troca de área de trabalho e nenhuma sessão é criada.
- **AC-002-07** _Dado_ o atalho mãos-livres configurado como `Win+Space`, _quando_ o pressiono, _então_ a gravação alterna e o layout de teclado do Windows **não** muda.
- **AC-002-08** (v1.1+) _Quando_ começo com `Ctrl+Win`, adiciono `Alt` após 150 ms e falo uma instrução, _então_ a sessão é tratada como Command Mode.
- **AC-002-09** _Dado_ que a internet caiu, _quando_ termino um ditado com provedor em nuvem sem fallback, _então_ a Flow Bar mostra erro, a sessão aparece no histórico como `failed` com áudio e "Tentar novamente" funciona quando a rede volta.
- **AC-002-10** _Quando_ digo "vou chegar em 5 minutos enviar" num chat, _então_ é inserido "Vou chegar em 5 minutos." seguido de Enter.
- **AC-002-11** _Dado_ limite de 1 min, _quando_ gravo em mãos-livres, _então_ aos 0:00 restantes a gravação encerra e o texto é inserido; aviso visível aos 60 s restantes.
- **AC-002-12** _Dado_ ditado comum em português, tradução global desligada e um modelo de tradução local compatível disponível, _quando_ aciono `transcribe_translate`, falo e encerro, _então_ a sessão produz inglês e o próximo ditado comum continua em português sem alteração das preferências globais.
- **AC-002-13** _Dado_ Turbo, modelo ausente ou não baixado como modelo efetivo da tradução, _quando_ aciono o atalho, _então_ não há captura, inferência em modelo incompatível, fallback para nuvem ou mutação do clipboard; há orientação localizada para escolher/baixar um modelo compatível.
- **AC-002-14** _Dado_ uma sessão traduzida em andamento, _quando_ cancelo ou mudo de janela antes da inserção no Windows, _então_ nenhum texto é inserido no novo destino e o clipboard anterior é preservado. Cancelamento e falha não habilitam tradução para sessões posteriores.

## Casos de borda

- Usuário solta as teclas em ordem diferente / uma tecla "presa" (evento de key-up perdido, ex.: após UAC) → timeout de sanidade: se o estado físico (`GetAsyncKeyState`) indicar que nenhuma tecla está pressionada, encerra.
- Trava de sessão (Win+L) durante gravação → cancelar e descartar.
- Sem microfone / permissão negada nas configurações de privacidade do Windows → erro com botão que abre `ms-settings:privacy-microphone`.
- Microfone em uso exclusivo por outro app → erro claro.
- Hook removido pelo SO (timeout) → watchdog reinstala e registra no log.

## Notas técnicas

- Hook: `SetWindowsHookExW(WH_KEYBOARD_LL)` numa thread com `GetMessageW`. Ignorar eventos injetados pelo próprio app (`LLKHF_INJECTED` + marca em `dwExtraInfo`).
- Estado físico das teclas mantido no matcher (conjunto de VKs pressionados), normalizando `LCtrl/RCtrl` → `Ctrl`, `LWin/RWin` → `Win`.
- Matcher sem efeitos colaterais (`fn feed(event, now) -> Vec<HotkeyAction>`), para ser testado conforme `rules/rust/testing.md` com sequências de eventos e tempos.
- Para os testes E2E exigidos por `rules/common/testing.md` (skill `windows-desktop-e2e`), builds de teste aceitam um arquivo WAV como fonte de áudio (feature flag de compilação, ausente no release).
