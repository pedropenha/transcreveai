# F008 — Detecção de reunião e popup

**Status**: Draft · **Release**: v1 · **Depende de**: F001 (slot de toast), F009

## Contexto

Ao entrar numa chamada (Zoom, Teams, Meet, Slack, Discord…), o app mostra um toast discreto oferecendo iniciar o Notetaker (prints 3 e 4 em [research](../../product/research-wispr-flow.md#o-que-os-prints-mostram)). Tudo é detectado **localmente**, sem ler áudio antes da confirmação.

## Histórias

- **US-008-01** Quero ser lembrado de gravar notas quando entro numa reunião.
- **US-008-02** Quero iniciar com um clique, sem abrir o app.
- **US-008-03** Quero que certas reuniões/apps iniciem sozinhos e outros nunca perguntem.
- **US-008-04** Quero que a gravação pare sozinha quando a chamada terminar.

## Sinais de detecção (Windows)

| Sinal                                   | Fonte                                                                                                                                                                                                             | Uso                                             |
| --------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ----------------------------------------------- |
| **S1 — Mic em uso por outro processo**  | Registro `HKCU\Software\Microsoft\Windows\CurrentVersion\CapabilityAccessManager\ConsentStore\microphone` (subchaves de apps empacotados e `NonPackaged\<caminho com #>`): `LastUsedTimeStop == 0` ⇒ em uso agora | Sinal principal; identifica o executável.       |
| **S2 — Janela/processo conhecido**      | Lista de regras (`meeting_app_rules`): exe + regex de título                                                                                                                                                      | Identifica o app e rotula (ex.: "Google Meet"). |
| **S3 — Sessão de áudio de saída ativa** | WASAPI `IAudioSessionManager2` (sessões `Active` por PID)                                                                                                                                                         | Aumenta a confiança (P1).                       |

### Regras embutidas (editáveis)

Na **v1** o conjunto embutido é **Zoom, Teams, Meet (navegador) e Webex** (ADR-0002); as demais linhas entram na v1.1+. Regras adicionadas pelo usuário funcionam já na v1.

| Rótulo          | Exe                                                               | Título (regex, aproximado — validar na implementação) | Release |
| --------------- | ----------------------------------------------------------------- | ----------------------------------------------------- | ------- |
| Zoom            | `Zoom.exe`                                                        | `Zoom Meeting\|Reunião Zoom\|Zoom Workplace`          | v1      |
| Microsoft Teams | `ms-teams.exe`, `Teams.exe`                                       | reunião/chamada ativa (janela de chamada)             | v1      |
| Google Meet     | `chrome.exe`, `msedge.exe`, `firefox.exe`, `brave.exe`, `arc.exe` | `^Meet -\|meet\.google\.com`                          | v1      |
| Webex           | `CiscoCollabHost.exe`, `webexmta.exe`                             | —                                                     | v1      |
| Slack Huddle    | `slack.exe`                                                       | `Huddle`                                              | v1.1+   |
| Discord         | `Discord.exe`                                                     | (mic em uso basta)                                    | v1.1+   |
| Whereby / Jitsi | navegadores                                                       | `Whereby\|Jitsi Meet`                                 | v1.1+   |

## Requisitos funcionais

### Detecção

- **FR-008-01** Monitorar S1 com `RegNotifyChangeKeyValue` na árvore da chave (fallback: polling a cada 2 s). Ignorar o próprio executável.
- **FR-008-02** **Reunião detectada** quando: S1 aponta um processo **e** (esse processo casa uma regra `ask`/`auto_start` **ou** — para navegadores — alguma janela do processo casa o título da regra, atualmente ou nos últimos 10 min), estável por ≥ 5 s (debounce).
- **FR-008-03** "Detectar qualquer chamada" (padrão desligado): S1 sozinho por ≥ 10 s em processo que não seja navegador nem esteja na lista de ignorados (gravadores de voz, apps de ditado, o próprio app) também dispara, rotulado com o nome do produto do executável.
- **FR-008-04** **Fim da reunião** quando: o processo libera o mic por ≥ 15 s, **ou** o processo termina, **ou** (navegador) nenhuma janela casa mais o título e o mic foi liberado.
- **FR-008-05** Cada detecção tem `detection_id`; a mesma reunião não gera dois toasts (chave: exe + PID + início).
- **FR-008-06** Enquanto o **próprio** app estiver gravando (ditado/reunião), mudanças de S1 causadas por ele são ignoradas.

### Toast (UX)

- **FR-008-07** Toast compacto, ancorado acima da Flow Bar (ou canto inferior direito, configurável): ícone do app (ou câmera genérica), "Reunião detectada", "● Agora" e o rótulo do app. Não-ativável (não rouba foco).
- **FR-008-08** Ao passar o mouse, expande: **✕** (dispensar), botão primário **"Iniciar Notetaker"** e **▾** com:
  - Iniciar só com microfone
  - Sempre iniciar automaticamente para <App>
  - Nunca perguntar para <App>
  - Ignorar esta reunião
- **FR-008-09** "Iniciar Notetaker" → chama `meeting_start` (F009) com app/rótulo; o toast vira confirmação "Gravando · <App>" por 3 s e some; a Flow Bar passa a `meeting_recording`.
- **FR-008-10** Sem interação, o toast recolhe após 60 s para um indicador pequeno na Flow Bar (ponto âmbar) que reabre o toast ao hover, até a reunião terminar.
- **FR-008-11** Som de notificação opcional (padrão desligado).
- **FR-008-12** Não mostrar toasts com "Não perturbe"/foco do Windows ativo ou em tela cheia, exceto se o usuário optar; a detecção continua (indicador na Flow Bar).

### Automação

- **FR-008-13** "Iniciar automaticamente" (global ou por regra `auto_start`): inicia a gravação sem perguntar e mostra toast "Gravando · <App>" com botão "Parar".
- **FR-008-14** Auto-stop (padrão ligado): no fim da reunião (FR-008-04), se a gravação foi iniciada por detecção ou pertence àquele app, mostrar toast "A reunião terminou — finalizando em 15 s" com **"Continuar gravando"**; sem ação, encerra (F009).
- **FR-008-15** Configurações: Detectar reuniões (on/off) · Detectar qualquer chamada · Iniciar automaticamente · Auto-stop · Posição do toast · Lista de regras por app (ask / auto / ignore) com adicionar/editar/remover.

## Requisitos não funcionais

- **NFR-008-01** Detecção ≤ 7 s após o app de reunião abrir o mic (p95).
- **NFR-008-02** Custo em repouso ≤ 0,5 % CPU (baseado em notificação, não polling agressivo).
- **NFR-008-03** Nenhum áudio é lido para detectar reuniões.

## Critérios de aceitação

- **AC-008-01** _Dado_ a detecção ligada, _quando_ entro numa reunião do Google Meet no Chrome, _então_ em ≤ 7 s aparece o toast "Reunião detectada · Google Meet" e o foco continua no Chrome.
- **AC-008-02** _Quando_ passo o mouse no toast, _então_ ele mostra ✕, "Iniciar Notetaker" e ▾; _quando_ clico em "Iniciar Notetaker", _então_ a gravação começa e a Flow Bar mostra o cronômetro.
- **AC-008-03** _Dado_ "Nunca perguntar para Discord", _quando_ entro numa chamada no Discord, _então_ nenhum toast aparece.
- **AC-008-04** _Dado_ auto-start para Zoom, _quando_ entro numa reunião Zoom, _então_ a gravação começa sem clique e o toast de gravação aparece.
- **AC-008-05** _Dado_ uma gravação iniciada por detecção, _quando_ saio da chamada, _então_ após 15 s a gravação para e o processamento começa; _se_ clico em "Continuar gravando" antes, _então_ ela continua.
- **AC-008-06** _Quando_ uso o próprio ditado do app, _então_ nenhum toast de reunião aparece.
- **AC-008-07** _Dado_ que dispensei o toast (✕), _então_ ele não reaparece para a mesma reunião; numa nova reunião, reaparece.
- **AC-008-08** Matriz manual: Zoom, Teams (novo), Meet (Chrome/Edge), Webex — registrar detecção e fim. Na v1 é roteiro manual parcial (T-069); Slack/Discord/WhatsApp entram na v1.1+.

## Casos de borda

- Meet em aba não ativa: título da janela não mostra "Meet" → a memória de 10 min (FR-008-02) cobre o caso comum de trocar de aba após entrar. P2: extensão de navegador que informa URL/estado da chamada.
- Navegador usando mic para outra coisa (ditado web, gravação) → só dispara com título de reunião (ou "qualquer chamada", que exclui navegadores).
- Teams mantém o mic aberto em pré-visualização da chamada → debounce + título/estado da janela; aceitável disparar um pouco antes.
- Múltiplas reuniões simultâneas → só uma gravação por vez; segundo toast oferece "Trocar para esta reunião".

## Notas técnicas

- `platform::MicUsageMonitor` emite `MicUsage { exe_path, pid?, in_use, since }`; `meeting::Detector` é uma máquina de estados pura alimentada por esses eventos + snapshots de janelas (`EnumWindows` + `GetWindowTextW` + `GetWindowThreadProcessId`) → testes com timelines simuladas.
- macOS (v1.0): `kAudioDevicePropertyDeviceIsRunningSomewhere`, objetos de processo de áudio (`kAudioHardwarePropertyProcessObjectList`, macOS 14+), títulos via Accessibility.
