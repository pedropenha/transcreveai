# F001 — Flow Bar

**Status**: Draft · **Release**: v1 · **Depende de**: F002 (sessão), F009 (botão de reunião)

## Contexto

Ponto de entrada visual sempre presente, independente do app em uso. Em repouso é quase invisível; ao passar o mouse, mostra **duas ações**: _Ditar_ e _Notas de reunião_. Durante o ditado, dá feedback de que o app está ouvindo. Ver prints 1 e 2 em [research](../../product/research-wispr-flow.md#o-que-os-prints-mostram).

## Histórias

- **US-001-01** Como usuário, quero ver discretamente que o app está pronto, sem atrapalhar o que estou fazendo.
- **US-001-02** Como usuário, quero passar o mouse na barra e clicar para ditar ou começar notas, sem decorar atalhos.
- **US-001-03** Como usuário, quero ver que o app está me ouvindo (e o nível do meu microfone) e quando está processando.
- **US-001-04** Como usuário, quero esconder a barra durante apresentações ou jogos e movê-la se ela cobrir um botão.

## Requisitos funcionais

### Estados visuais

| Estado                     | Visual de referência (prints do Wispr)                                                                                                                  | Tamanho aprox. |
| -------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------- | -------------- |
| `hidden`                   | Nada                                                                                                                                                    | —              |
| `idle`                     | Pílula arredondada escura, discreta, com borda sutil (print 1)                                                                                          | 48 × 8 px      |
| `hover`                    | Cápsula com 2 botões circulares 28 px: 🎤 **Ditar** e ◉ **Notas de reunião**; tooltip acima do botão focado com nome + atalho (ex.: "Ditar Ctrl + Win") | 88 × 36 px     |
| `recording`                | Cápsula com 7 barras de onda animadas pelo nível do mic, botão ✕ (cancelar) à esquerda e ■ (parar) à direita; em PTT, os botões aparecem só no hover    | 128 × 36 px    |
| `recording_command`        | Igual a `recording` com rótulo/ícone "Comando" (cor de destaque diferente)                                                                              | 150 × 36 px    |
| `processing`               | Três pontos pulsando ou shimmer                                                                                                                         | 64 × 24 px     |
| `done`                     | ✓ breve (600 ms), volta a `idle`                                                                                                                        | 48 × 24 px     |
| `error`                    | Contorno vermelho + ícone ⚠; hover mostra mensagem curta; clique abre detalhes/"Tentar novamente"                                                      | 64 × 24 px     |
| `meeting_recording`        | Ponto vermelho pulsando + cronômetro `12:34` ao lado da pílula; clique abre a janela da reunião                                                         | 96 × 24 px     |

Os tamanhos aproximam o Wispr. Cores, tipografia, raios e animações saem da direção de design (tarefa T-008), feita conforme `rules/web/design-quality.md` usando os prints do Wispr como referência.

- **FR-001-01** A barra aparece por padrão centralizada horizontalmente na borda inferior da **área de trabalho** do monitor (acima da barra de tarefas), com margem de 8 px.
- **FR-001-02** Ao entrar com o mouse (atraso 120 ms), anima de `idle` para `hover` em ≤ 150 ms (ease-out). Ao sair (atraso 400 ms), volta.
- **FR-001-03** Clique em 🎤 inicia uma sessão de ditado **mãos-livres**; novo clique (ou ■) encerra e insere. O texto vai para a janela que estava em foco antes do clique.
- **FR-001-04** Clique em ◉ inicia uma sessão de Notetaker (F009), como no Wispr.
- **FR-001-05** Durante `recording`, as barras refletem o RMS do microfone recebido via `audio://level` (30 Hz), com suavização.
- **FR-001-06** O botão ✕ cancela a sessão (descarta áudio); ■ para e processa.
- **FR-001-07** Clique direito abre menu nativo:
  - Ocultar por 15/30/60 min (soneca) · Ocultar até reiniciar o app
  - Microfone ▸ (lista de dispositivos, marcado o atual)
  - Idioma ▸ (Automático, Português, Inglês, …)
  - Colar última transcrição
  - Abrir Hub · Configurações · Sair
- **FR-001-08** A barra pode ser **arrastada** ao longo da borda inferior e encaixada na borda esquerda ou direita (orientação vertical). A posição (borda + deslocamento relativo 0–1) é salva por configuração de monitores.
- **FR-001-09** Multi-monitor: por padrão, segue o monitor da **janela em primeiro plano**; opções: seguir o cursor, fixo no monitor principal.
- **FR-001-10** Visibilidade (configuração): _Sempre_ · _Só durante gravação_ · _Nunca_ (atalhos continuam funcionando; feedback via ícone da bandeja).
- **FR-001-11** Com "Ocultar em tela cheia" (padrão ligado), a barra some quando a janela em primeiro plano cobre o monitor inteiro (jogos, apresentações, vídeo), exceto se uma gravação estiver ativa.
- **FR-001-12** Tema segue o Windows (claro/escuro) e respeita "Efeitos de animação" desligado (sem animações, só troca de estado).
- **FR-001-13** Sons de início/fim de gravação (curtos, suaves), **ligados por padrão**, desativáveis.
- **FR-001-14** O slot acima da barra é usado para **toasts** (F008) sem sobrepor a barra.

## Requisitos não funcionais

- **NFR-001-01** A janela **nunca** recebe foco: clicar nela não altera a janela em primeiro plano.
- **NFR-001-02** Área transparente da janela é _click-through_: cliques fora da pílula visível passam para o app de baixo.
- **NFR-001-03** Feedback visual ≤ 50 ms após o atalho.
- **NFR-001-04** CPU da janela em `idle` ≈ 0 % (sem animações rodando em repouso).
- **NFR-001-05** Sempre acima de outras janelas, inclusive após o usuário clicar na barra de tarefas (reaplicar `HWND_TOPMOST` quando necessário).

## Critérios de aceitação

- **AC-001-01** _Dado_ o Notepad em foco com cursor no texto, _quando_ clico em 🎤, falo "teste um dois" e clico em ■, _então_ "Teste um dois." aparece no Notepad e `GetForegroundWindow()` continuou sendo o Notepad o tempo todo.
- **AC-001-02** _Dado_ a barra em `idle`, _quando_ clico num botão de um app logo acima da área transparente da janela da barra (fora da pílula), _então_ o clique chega ao app.
- **AC-001-03** _Dado_ o mouse sobre a barra, _então_ aparecem exatamente 2 botões e o tooltip do botão sob o cursor mostra o atalho configurado atualmente.
- **AC-001-04** _Dado_ que arrastei a barra para a borda direita, _quando_ reinicio o app, _então_ ela reaparece na borda direita.
- **AC-001-05** _Dado_ um vídeo em tela cheia no navegador, _então_ a barra fica oculta; _quando_ inicio um ditado por atalho, _então_ ela aparece durante a gravação.
- **AC-001-06** _Dado_ "Ocultar por 60 min", _então_ a barra some e reaparece após 60 min ou ao escolher "Mostrar Flow Bar" na bandeja; o mesmo vale para 15 e 30 min.
- **AC-001-07** _Dado_ uma gravação com o microfone mudo, _então_ as barras ficam planas; ao falar, variam visivelmente.
- **AC-001-08** _Dado_ um erro de transcrição (ex.: modelo ausente), _então_ a barra entra em `error`, o hover mostra a causa e o clique oferece "Tentar novamente".

## Casos de borda

- Barra de tarefas oculta automaticamente / em outra borda → usar a _work area_ do monitor (`SystemParametersInfo(SPI_GETWORKAREA)`/`MonitorInfo.rcWork`).
- Mudança de DPI/escala ou desconexão de monitor → reposicionar no monitor válido mais próximo.
- Explorer reiniciado → reaplicar topmost e ícone da bandeja.
- Sessão bloqueada (Win+L) → ocultar; nenhuma gravação pode começar na tela de bloqueio.

## Notas técnicas

- Janela Tauri: `transparent: true`, `decorations: false`, `always_on_top: true`, `skip_taskbar: true`, `focusable: false`, `shadow: false`. No Windows, garantir estilos `WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW | WS_EX_TOPMOST` via `windows` crate.
- **Tamanho fixo** (ex.: 360 × 140 px, âncora inferior) em vez de redimensionar por animação (evita flicker do WebView2). A animação é CSS dentro da janela.
- Click-through: `set_ignore_cursor_events(true)` por padrão. O núcleo consulta a posição do cursor (~30 Hz somente quando o cursor está a ≤ 60 px do retângulo da janela) e desliga o click-through quando o cursor entra no retângulo **da pílula visível** (informado pelo frontend via `flowbar_set_hover`/bounds).
- Fullscreen: comparar o retângulo da janela em primeiro plano com o do monitor; alternativamente `SHQueryUserNotificationState` (`QUNS_BUSY`, `QUNS_RUNNING_D3D_FULL_SCREEN`, `QUNS_PRESENTATION_MODE`).
- Nível do microfone a 30 Hz não passa por estado do React a cada frame (atualização de alta frequência — `rules/react/patterns.md`); animação só em propriedades compostas pela GPU (`transform`/`opacity`, `rules/web/performance.md`).
- Ícones: conjunto próprio (ex.: Lucide), sem reproduzir a identidade visual do Wispr.
