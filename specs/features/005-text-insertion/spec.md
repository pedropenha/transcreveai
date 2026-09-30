# F005 — Inserção de texto no app em foco

**Status**: Draft · **Release**: MVP · **Depende de**: F002, F004

## Contexto

O texto final precisa aparecer onde o cursor está, em qualquer app (nativo, Electron, navegador, terminal, IDE, desktop remoto), sem perder o conteúdo da área de transferência do usuário.

## Histórias

- **US-005-01** Quero que o texto apareça no cursor como se eu tivesse digitado.
- **US-005-02** Quero que minha área de transferência continue com o que eu tinha copiado.
- **US-005-03** Quero desfazer a inserção com um único `Ctrl+Z`.
- **US-005-04** Quero que funcione em desktop remoto e em apps que bloqueiam colar.

## Métodos

| Método               | Como                                                                                                           | Quando                                                            |
| -------------------- | -------------------------------------------------------------------------------------------------------------- | ----------------------------------------------------------------- |
| `paste` (padrão)     | Salva clipboard → escreve texto → `Ctrl+V` via `SendInput` → restaura clipboard                                | Quase todos os apps; 1 operação de desfazer.                      |
| `paste_shift_insert` | Igual, com `Shift+Insert`                                                                                      | Terminais legados / apps onde `Ctrl+V` tem outro significado.     |
| `type`               | `SendInput` com `KEYEVENTF_UNICODE` caractere a caractere                                                      | Apps que bloqueiam colar; RDP/Citrix sem clipboard compartilhado. |
| `clipboard_only`     | Só copia e avisa "Texto copiado — cole com Ctrl+V"                                                             | Alvo não alcançável (elevado, tela segura).                       |
| `auto`               | `paste`, trocando para `type` em exes conhecidos (`mstsc.exe`, `wfica32.exe`, `vmconnect.exe`, `CDViewer.exe`) | Padrão dos perfis.                                                |

## Requisitos funcionais

- **FR-005-01** Antes de injetar, **esperar a liberação física** de todas as teclas do atalho (até 500 ms, consultando `GetAsyncKeyState`); se ainda houver modificadores pressionados, injetar key-ups sintéticos. Evita que `Win` + `V` abra o histórico da área de transferência ou `Ctrl+Alt+V` dispare atalhos.
- **FR-005-02** Salvar o conteúdo atual do clipboard (todos os formatos que for possível ler: texto, HTML, RTF, imagem, lista de arquivos). Se não for possível salvar algum formato, avisar no log e seguir.
- **FR-005-03** Escrever o texto (`CF_UNICODETEXT`) marcado para **não entrar no histórico/nuvem do clipboard do Windows** (formatos `ExcludeClipboardContentFromMonitorProcessing` e `CanIncludeInClipboardHistory = 0`).
- **FR-005-04** Enviar a combinação de colar; aguardar `paste_delay_ms` (padrão 120 ms, configurável por perfil) e restaurar o clipboard **somente se** `GetClipboardSequenceNumber` indicar que ninguém mais o alterou depois de nós.
- **FR-005-05** Modo `type`: taxa configurável (padrão sem atraso; 5 ms/char para RDP); `\n` enviado como `Enter` ou `Shift+Enter` conforme `newline_mode` do perfil (chats onde Enter envia).
- **FR-005-06** Se `press_enter` estiver ligado (F002/F004), enviar `Enter` após a inserção (+50 ms).
- **FR-005-07** **Janela elevada**: se o processo alvo tiver integridade maior que a do app (UIPI), não tentar injetar; usar `clipboard_only` com aviso "Não dá para digitar numa janela de administrador; o texto foi copiado".
- **FR-005-08** Sem janela válida em foco (área de trabalho, tela bloqueada, UAC) → `clipboard_only` + aviso.
- **FR-005-09** Janela em foco diferente da do início da sessão: inserir na janela **atual** (comportamento natural de "onde o cursor está"). Opção "Se a janela mudou, apenas copiar" (padrão desligada).
- **FR-005-10** Inserções de sessões concorrentes são serializadas (FIFO) — nunca duas colagens simultâneas.
- **FR-005-11** Resultado da inserção registrado no histórico (`inserted`, `copied`, `failed`) com método usado e tempo.
- **FR-005-12** P2: verificar via UI Automation se o elemento focado é editável e confirmar que o texto entrou; senão, cair para `clipboard_only`.

## Requisitos não funcionais

- **NFR-005-01** Inserção (`paste`) ≤ 200 ms fim a fim, incluindo restauração.
- **NFR-005-02** Eventos injetados marcados em `dwExtraInfo` para que o próprio hook (F002) os ignore.

## Critérios de aceitação

- **AC-005-01** _Dado_ "ABC" no clipboard, _quando_ dito "olá" no Notepad, _então_ "Olá." é inserido e, depois, o clipboard contém "ABC".
- **AC-005-02** _Dado_ uma imagem no clipboard, _quando_ dito algo, _então_ a imagem continua no clipboard depois.
- **AC-005-03** _Quando_ dito algo no Word e aperto `Ctrl+Z` uma vez, _então_ todo o texto ditado some.
- **AC-005-04** _Dado_ o histórico do clipboard do Windows ligado (Win+V), _então_ o texto ditado **não** aparece nele.
- **AC-005-05** _Dado_ um PowerShell "Executar como administrador" em foco, _então_ o texto não é digitado, fica no clipboard e aparece o aviso.
- **AC-005-06** A inserção funciona com o método `auto` em: Notepad, Word, Outlook, Chrome (Gmail, Google Docs), Edge, Slack, Teams, WhatsApp Desktop, VS Code, Cursor, Windows Terminal, cmd, Notion, Obsidian, Discord e RDP (`mstsc`). Automatizado com a skill `windows-desktop-e2e` onde o app expõe UI Automation; o restante (ex.: RDP) fica em roteiro manual.
- **AC-005-07** _Dado_ o perfil do Slack com `newline_mode = shift_enter` e método `type`, _quando_ dito "linha um nova linha linha dois", _então_ aparecem duas linhas na caixa sem enviar a mensagem.
- **AC-005-08** _Quando_ o usuário ainda segura `Ctrl` ao fim do PTT, _então_ nenhum atalho indesejado (`Ctrl+Win+V`, etc.) é disparado.

## Casos de borda

- Outro app monitorando/travando o clipboard (`OpenClipboard` falha) → retry 5× a cada 20 ms; depois `type`.
- Texto muito longo em `type` (> 5.000 chars) → avisar lentidão e preferir `paste`.
- Layouts de teclado não-US: `KEYEVENTF_UNICODE` independe de layout; para `Ctrl+V` usar o VK de `V` (independe de layout no Windows).
- Emojis/caracteres fora do BMP em `type` → enviar pares substitutos UTF-16.

## Notas técnicas

- Implementação em `platform/windows/injector.rs` atrás do trait `TextInjector { fn insert(&self, text, opts) -> Result<InsertOutcome> }`.
- Detectar elevação: `OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION)` + `GetTokenInformation(TokenIntegrityLevel)` do alvo vs. o próprio processo.
