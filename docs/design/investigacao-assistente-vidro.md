# Assistente — proposta Vidro & Anil e investigação

Data: 2026-10-02. Status: proposta para avaliação; aplicação não alterada.

Registro histórico da investigação anterior à implementação. Proposta aprovada pelo usuário em 2026-10-02; acompanhamento da implementação em [assistant-validation.md](./assistant-validation.md).

Prévia: [proposta-assistente-vidro.html](./proposta-assistente-vidro.html).

## Direção proposta

Uma janela flutuante de 440 × 640 px lógicos, com raio de 24 px, vidro fosco moderado, contorno fino e sombra difusa. Dimensões devem ser limitadas à área útil da tela; em telas pequenas, reduzir altura e permitir rolagem da conversa. A implementação atual é fixa em 420 × 560 px.

Instrument Sans nos controles e mensagens; Instrument Serif somente nas boas-vindas. Anil nos estados ativos e no envio. A resposta da IA aparece diretamente na superfície; a mensagem do usuário usa uma bolha discreta. Cabeçalho com três ações separadas, alvos de 36 × 36 px e ícones de 18 px. Provedor em linha própria, sem apertar o título. Campo de entrada permanente no rodapé.

Vidro claro: superfície `rgba(252,251,247,.78)`, texto `#262b36`, secundário `#606775`, anil `#3c4f98`. Vidro escuro: superfície `rgba(30,35,46,.78)`, texto `#f0f1f4`, secundário `#b5bdce`, anil `#b6c6ff`. Blur ilustrativo de 28 px, saturação 125%. Estes são tokens candidatos específicos do assistente, não uma alteração global dos overlays. Contraste deve ser medido contra fundos reais na implementação, aumentando opacidade se necessário.

Alternativa “Reduzir transparência” usa superfície sólida. Também respeitar redução de movimento. A proposta introduz um assistente claro e escuro; a identidade aprovada atualmente define overlays sempre escuros. Registrar essa exceção no ADR-0003 e ajustar NFR-012-04 quando a proposta for aprovada.

Referência de material: [Apple HIG — Materials](https://developer.apple.com/design/human-interface-guidelines/materials). A inspiração é profundidade, separação dos controles e translucidez; não pressupõe que o Windows ofereça o material Liquid Glass da Apple.

O HTML simula vidro sobre um fundo ilustrativo na mesma página. `backdrop-filter` sozinho não comprova desfoque dos outros aplicativos atrás de uma janela WebView2. Para isso, avaliar efeito nativo Acrylic no Windows e material correspondente no macOS, mantendo alternativa sólida. A janela atual já é transparente, mas não configura efeito nativo de vidro. Consultar [Tauri — Window customization](https://v2.tauri.app/learn/window-customization/) e [API de efeitos de janela](https://v2.tauri.app/reference/javascript/api/namespacewindow/). Validar desempenho ao arrastar e composição nas versões de Windows suportadas antes de escolher o efeito definitivo.

## Investigação dos controles

Inspeção do frontend, comandos registrados, capabilities, sessão Rust e caminho de janela Windows. Validação focada: `bunx playwright test tests/assistant.spec.ts` — **10 testes passaram**. Os testes usam IPC Tauri simulado em Chromium; não demonstram que os cliques chegam à janela nativa.

| Ponto         | Confirmado no código                                                                                                                                                                                               | Problema ou limite                                                                                                                                                                                                                                                                             | Caminho proposto                                                                                                                                                                                              |
| ------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Fechar        | `AssistantPanel.tsx` chama `assistantClose`; Rust oculta a janela, conserva conversa e aborta ditado roteado. Existe permissão de ocultar e fallback frontend.                                                     | `hide_native` ignora o resultado de `window.hide()`. O comando retorna sucesso mesmo que ocultar falhe; nesse caso o fallback frontend não roda. Agendamento no thread principal também tem resultado ignorado. Isso é uma lacuna de diagnóstico confirmada, não a causa comprovada do relato. | Propagar falhas reais da ocultação/agendamento; registrar erro sem conteúdo da conversa; confirmar `open=false` e invisibilidade nativa. Manter fechar utilizável com e sem pin, durante resposta e ditado.   |
| Nova conversa | Comando registrado; aborta resposta pendente, invalida geração, limpa mensagens, fila e erro e publica estado.                                                                                                     | Não interrompe nem limpa explicitamente o ditado roteado em andamento. Um ditado antigo pode concluir e entrar na conversa recém-limpa. Em uma sessão já vazia, o botão não dá retorno visual adicional.                                                                                       | Abortar ditado antigo junto com a resposta, limpar rascunho e prévia, apresentar boas-vindas e devolver foco ao campo após ação explícita. Testar resposta tardia e transcrição tardia.                       |
| Pin           | Grava `assistant_panel_pinned` nas configurações, emite estado; frontend e backend bloqueiam arraste.                                                                                                              | A janela já é sempre topmost. Pin significa **travar posição**, não ligar “sempre por cima”. O toggle calcula o próximo valor a partir do último estado recebido; cliques antes da confirmação podem mandar o mesmo valor. Falhas aparecem apenas no console.                                  | Nome “Fixar posição”, tooltip “Desafixar posição”, indicação persistente e `aria-pressed`; serializar comandos ou bloquear somente o toggle enquanto pendente; erro visível e estado confirmado pelo backend. |
| Digitar       | Não há `textarea`, input nem compositor no painel. `assistant_send(text)` já existe no binding, está registrado e aceita texto não vazio; usa a mesma sessão e fila do ditado. Janela Windows é `focusable(true)`. | Ausência de campo é deliberada em F012, “Fora” e FR-012-13. Não é falha do teclado.                                                                                                                                                                                                            | Adicionar compositor React usando `assistantSend`; Enter envia, Shift+Enter quebra linha, respeitar IME; desabilitar envio vazio/sem provedor; preservar texto se comando falhar e dar erro visível.          |

### Por que ainda não atribuí uma causa aos cliques

O cabeçalho possui arraste por pointer events e handlers de botão que executam no `pointerdown`, impedem o comportamento padrão e deduplicam o `click`. Os três controles barram propagação de `mousedown`, portanto não passam pelo handler de foco do painel; o Windows pode ainda ativar a janela automaticamente. O código tenta contornar cliques perdidos na ativação do WebView2, mas os testes de Chromium não reproduzem essa transição nativa.

O caminho Windows configura a janela como focusable e usa `SWP_NOACTIVATE` apenas para mostrá-la. O helper `force_overlay_topmost` não aplica `WS_EX_NOACTIVATE` ao assistente. Não há evidência estática de uma janela intencionalmente sem cliques nem de comandos ausentes. Não recomendar remover proteção de foco da Flow Bar: ela é outra janela e deve continuar não ativável.

Na implementação, testar primeiro clique após abrir sem foco, clique após ativar, pressões rápidas/longas, teclado e arraste. Capturar o encadeamento evento → IPC → estado → janela, sem registrar prompts. Só simplificar os handlers depois de reproduzir a falha, preservando exatamente uma ação por gesto. Uma ação que falha precisa de retorno visível; o console não resolve um botão aparentemente morto.

## Comportamento do compositor proposto

- Texto digitado é rascunho local. Só enviar para o provedor ao acionar Enviar/Enter. Limpar apenas após aceite do comando, sem apagar texto que tenha sido editado enquanto o envio aguardava.
- Durante resposta, manter rascunho editável e mostrar Parar; nesta proposta o envio digitado aguarda o término. O backend já enfileira ditados, e esse comportamento deve ser preservado e mostrado separadamente.
- Voz continua disponível pelo atalho existente. A proposta acrescenta botão “Ditar” no compositor; conectar ao pipeline real, sem simular envio via outra janela. O atalho atual continua encerrando e enviando o ditado. Não mudar silenciosamente para um ditado que só preenche o campo.
- Fechar conserva conversa e rascunho em memória; reabrir restaura. Nova conversa limpa ambos. Não acrescentar persistência de chat em disco.
- Esc durante resposta cancela; fora desse estado oculta o painel. Foco do campo somente após clique/ação explícita; abrir o painel não deve capturar teclas de outro app.
- Sem provedor, permitir preparar texto, mas impedir envio e mostrar acesso às configurações. Falha mantém a mensagem e permite tentar novamente. Sinalizar o provedor efetivo e as regras de envio de contexto local já existentes.

## Plano para implementação após avaliação

1. Atualizar F012 para teclado + voz, nova conversa durante ditado e semântica explícita do pin; registrar a exceção visual do assistente no ADR.
2. Reproduzir falhas nativas e escrever testes que falhem para o comportamento reproduzido. Cobrir limpeza de ditado, erro real de ocultação e comandos pendentes.
3. Implementar compositor reutilizando `assistant_send`; fontes existentes e strings em pt-BR/en. Preservar fila, cancelamento e guard de geração do backend.
4. Aplicar a nova composição visual e avaliar efeito de vidro nativo com alternativa sólida. Testar claro/escuro, fundos claros/escuros, DPI 100/125/150/200%, teclado, leitores de tela e axe.
5. Validar o aplicativo Windows real: primeiro clique, foco, fechar e reabrir, nova conversa, pin persistido, arraste entre monitores, digitar/enviar, erro e cancelar. Capturar apenas a janela autorizada, nunca a tela inteira.
6. Executar os gates do repositório e revisão ECC; Rust test/clippy se houver alterações no backend. Commit somente da implementação validada e autorizada.

## Arquivos examinados

- `src/assistant/AssistantPanel.tsx` — listeners, foco, gestos, ações e renderização sem compositor.
- `src/assistant/AssistantPanel.css` — painel quase opaco (98%), cabeçalho compacto, bolhas e rodapé somente de dica.
- `src/assistant/main.tsx`, `src/assistant/index.html` — tema/fontes e documento transparente.
- `src-tauri/src/commands/assistant.rs` — comandos de sessão, posição e pin.
- `src-tauri/src/assistant/panel.rs` — geometria, foco e ciclo de vida nativo.
- `src-tauri/src/assistant/turn.rs`, `state.rs` — envio, fila, geração, nova conversa e estado publicado.
- `src-tauri/src/overlay/win32.rs` — topmost sem ativação e estilos exclusivos da Flow Bar.
- `src-tauri/src/lib.rs`, `src/bindings.ts`, `src-tauri/capabilities/assistant.json` — registro, binding e permissão de ocultar.
- `specs/features/012-voice-assistant/spec.md`, `docs/adr/0003-identidade-visual-papel-e-anil.md`, `docs/design/proposta-ui.html` — decisões existentes.
- `tests/assistant.spec.ts` — cobertura de gestos, hidratação, pin e fallback de fechar com IPC simulado.

## Limites desta entrega

Nenhum componente, backend, spec aprovada ou configuração do app foi modificado. Não foi feita chamada real a provedor, teste de microfone ou teste nativo dos cliques. Não foi executada a suíte completa: esta entrega é uma proposta e investigação focada. A causa nativa dos botões ainda requer reprodução. A prévia tem interação demonstrativa para tema, transparência, estados, nova conversa, fechar/reabrir, pin e mensagens digitadas; não é o aplicativo pronto.
