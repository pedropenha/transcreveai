# Assistente — Vidro & Anil

Proposta aprovada em 2026-10-02. Checkout `C:\multimidia\ecc`, branch `integration/v1`. Implementação disponível no app de desenvolvimento; entrega técnica pendente dos itens abaixo.

## Mudanças

- Sem moldura externa: painel ocupa a janela, sem margem, borda ou sombra CSS; recorte nativo Windows acompanha os cantos arredondados e o DPI para não expor Acrylic fora do painel. Testes nativos verificam o recorte em escalas 1/1,5/2. Assistente em vidro claro/escuro, Instrument Sans/Serif locais, controles maiores e provedor separado. Acrylic no Windows respeita a preferência de transparência do sistema; alternativa CSS e opção persistente Reduzir transparência. A opacidade foi aumentada em relação ao protótipo para conservar legibilidade.
- Campo permanente para digitar: Enter envia, Shift+Enter quebra linha, composição IME não envia prematuramente. Bloqueia envio vazio, sem provedor ou durante ditado/resposta. Preserva rascunhos em falhas e novas edições durante comandos pendentes.
- Ditar/Parar e enviar usa a rota do assistente. Reservas identificam a sessão para impedir que transcrições atrasadas entrem em outra conversa ou que ditados comuns sejam enviados ao assistente.
- Fechar/Esc aguardam o comando nativo e têm fallback de ocultação. Nova conversa cancela resposta/ditado e limpa somente o rascunho anterior. Pin fixa a posição e aguarda confirmação, preservando eventos mais recentes.
- Dimensões 440 × 640 px lógicos, limitadas à área útil do monitor. Traduções pt-BR/en; F012 e ADR-0003 atualizados.

## Verificação concluída

- TDD: campo ausente, perda de rascunho novo, snapshot atrasado, reservas de ditado e reinício após estados terminais tiveram regressões RED antes das correções GREEN.
- TypeScript, lint (zero erros; 15 avisos existentes), 23 módulos de testes unitários frontend, sincronização das 962 chaves de tradução e Prettier passaram.
- Playwright: a versão anterior passou integralmente nos 191 testes. Após remover a moldura, os 24 testes focados do assistente passaram. A última execução integral teve 190 aprovados e duas falhas de capturas do Dicionário; os dois casos passaram na repetição isolada com um worker. Execuções intermediárias também tiveram timeouts em toast/print. Ainda não há uma execução integral de 192 testes sem falhas registrada após esta correção.
- Axe no assistente em claro/escuro e material translúcido/sólido: zero violações nas condições testadas. Teclado, Enter, Shift+Enter, IME, Esc e controles cobertos por Playwright.
- Rust: **947 testes unitários passaram**, nenhum ignorado. `cargo fmt --check` e Clippy de todos os targets com warnings como erros passaram.
- Cobertura frontend real do Chromium, remapeada para as fontes e conferida por hash: painel 84,77% linhas/declarações, 80,76% funções e 89,3% branches; compositor 100% em todas as métricas; hook 98,21% linhas/declarações e 100% funções/branches. Gate mínimo de 80% passou em cada arquivo, sem exclusões de callbacks.
- Janela WebView2 do app real conectada ao backend: digitar, nova conversa e pin funcionaram; preferência de pin restaurada ao estado inicial. Fechar também confirmou a ocultação após a correção Windows. Nenhum prompt foi enviado a um provedor e nenhum áudio foi gravado nesse smoke.

## Pendências da entrega

- **Cobertura Rust mínima ainda não demonstrada.** LLVM instrumentou e executou os 945 testes, mas wrappers nativos e caminhos dependentes de `AppHandle` não foram exercitados suficientemente: `native.rs` 6,82% das linhas, `panel.rs` 22,84%, `state.rs` 33,90%, `turn.rs` 23,15%; coordinator/machine 85,87%. São valores por arquivo, não uma medição específica de linhas novas. Não representam cumprimento global da exigência de 80% do ECC. Relatório local em `coverage/assistant-rust.json` (ignorado pelo Git).
- Revisão ECC React final aprovou. Revisão Rust informou ausência de CRITICAL/HIGH e uma observação MEDIUM, corrigida com teste de reinício após Done/Error. As observações HIGH anteriores de TypeScript foram corrigidas, mas a nova revisão final TypeScript não terminou: o agente atingiu o limite de uso da conta. A confirmação final após a última correção Rust também ficou pendente por esse limite.
- **Fechar corrigido e confirmado na janela nativa.** A investigação posterior confirmou que o fluxo normal também usa `SetWindowPos` para abrir: o cache Tauri podia continuar oculto e `window.hide()` não retirar a janela. Agora o Windows usa `ShowWindow(SW_HIDE)` e confirma `IsWindowVisible`; teste nativo reproduz a abertura por posicionamento. Smoke WebView2 com backend real confirmou digitação, nova conversa, pin e ocultação após o botão fechar. Primeiro clique físico após perda de foco continua como verificação manual separada.
- Sem chamada real ao provedor, áudio real, cancelamento de subprocesso durante conversa, medição de latência, leitor de tela manual ou execução em macOS/Linux.
- Estabilizar/revalidar a suíte integral após esta correção: falhas intermitentes dos prints e reload durante execuções foram observados; não alterar requisitos para esconder falhas.
- Commit final pendente: não foi registrado como entrega completa enquanto cobertura nativa e revisões finais permanecem incompletas. O app de desenvolvimento continua rodando com as alterações.

## Capturas

- `docs/design/screens/assistant-native-no-frame.png`: superfície renderizada pela WebView2 do app real após remover a moldura; não é uma captura de toda a tela.

- `docs/design/screens/assistant-proposal-light.png`, `assistant-proposal-dark.png`: protótipo aprovado, fundo ilustrativo.
- `docs/design/screens/assistant-implemented-light.png`, `assistant-implemented-dark.png`: interface implementada em Chromium, 440 × 640.
- `docs/design/screens/assistant-implemented-light-solid.png`, `assistant-implemented-dark-solid.png`: alternativa sólida.

Capturas de navegador não comprovam composição Acrylic sobre outras janelas. Nenhuma captura da tela inteira foi feita. Os prints stage6 existentes foram preservados.

## Correção da moldura nativa (2026-10-02)

A primeira correção do CSS e do recorte não eliminou o frame branco. A captura enviada pelo usuário mostrou o problema fora da área da WebView. Medição Win32 com DPI aware confirmou HWND 660 × 960 px e área cliente apenas 638 × 947 px. A sombra nativa estava habilitada; o Tauri [documenta a borda branca em janelas sem decoração com shadow=true](https://docs.rs/tauri/latest/tauri/window/struct.WindowBuilder.html#method.shadow).

A sombra foi desativada somente no Windows, mantendo o Acrylic e o recorte arredondado. Após recompilar, a janela nativa e a área cliente mediram ambas 660 × 960 px. Captura por PrintWindow, exclusivamente do HWND Assistente pertencente ao processo Transcreve.ai, em `docs/design/screens/assistant-native-window-no-frame.png`: sem a faixa branca. Nenhuma captura da tela inteira. O ditado ativo foi cancelado antes da atualização; nada foi enviado ao provedor.

Após essa alteração, cargo fmt, os 947 testes Rust e Clippy de todos os targets passaram. Nenhum código frontend foi alterado nesta correção da sombra.

## Cantos e identidade do tema (2026-10-02)

O recorte nativo estava presente (GetWindowRgn = COMPLEXREGION, ponto 0/0 fora da região), mas o Acrylic pintava o fundo retangular nos cantos. Portanto, a correção anterior da sombra não resolveu todo o problema: a declaração de que a moldura tinha sido eliminada foi prematura. Acrylic removido; a superfície RGBA/CSS preserva translucidez, sem blur nativo sobre outras aplicações. Sombra Windows continua desativada e recorte arredondado mantido. A preferência Windows de transparência é consultada ao construir a janela.

Os aliases `--as-*` agora usam `--panel`, `--text`, `--muted`, `--accent` e os demais tokens semânticos do Hub. A paleta cinza azulada independente foi substituída pela paleta Papel & Anil. Teste de evento `theme-changed` confirma o vínculo em claro e escuro, sem mudança de tema persistido durante a verificação.

30 testes focados do assistente/overlays passaram; novo teste de troca de paleta também passou. 947 testes Rust, formatação e Clippy passaram. Captura somente do HWND do assistente via PrintWindow em `docs/design/screens/assistant-native-shared-theme.png`: fundo cinza retangular anterior removido. O pixel de canto capturado deixou de conter o cinza do Acrylic; o canto está fora da região nativa. A imagem de PrintWindow não equivale a uma medição de todo o compositor do desktop. As pendências de cobertura/revisão e suíte integral anteriores continuam abertas.

## Fixar na borda, com arrasto livre (2026-10-02)

Mudança de produto solicitada explicitamente: Fixar não trava a janela. O botão encaixa imediatamente na borda mais próxima da área útil do monitor. Segurar o título permite arrastar livremente, inclusive entre monitores; ao soltar com o encaixe habilitado, a janela vai à borda mais próxima e salva a posição. Desativar o botão devolve posicionamento livre. A restauração reaplica o encaixe após mudanças de monitor/área útil. A barra de tarefas fica fora da área útil.

TDD: teste de arrasto enquanto fixado falhou antes da alteração; testes Rust das quatro bordas, coordenadas negativas e painel maior que o monitor falharam antes do helper. Após a implementação, 25 testes Playwright do assistente passaram, 949 testes Rust passaram, TypeScript, traduções, testes unitários frontend, lint e Clippy passaram. Lint mantém 15 avisos anteriores. As verificações gerais de cobertura/revisão pendentes permanecem registradas acima.

Smoke nativo complementar: clique no botão Fixar na borda confirmou pinned=true; GetWindowRect e GetMonitorInfo confirmaram uma face da janela alinhada à borda da área útil do monitor. App reaberto e mantido rodando com o encaixe habilitado. Arrasto/release foram verificados por Playwright e geometria por testes Rust; gesto físico entre monitores continua sem validação manual nesta sessão.

## Barra de título aparecendo por trás da translucidez (2026-10-02)

O relato do usuário mostrou título Assistente, ícone e X nativos por trás da interface. Inspeção do HWND confirmou style=0x14C80000, incluindo WS_CAPTION e WS_SYSMENU, apesar de decorations(false)/shadow(false). O problema também está [registrado no Tauri](https://github.com/tauri-apps/tauri/issues/14859).

O assistente agora remove explicitamente WS_CAPTION, WS_SYSMENU, WS_THICKFRAME e botões de minimizar/maximizar, preservando os demais estilos, e aplica SWP_FRAMECHANGED sem ativar, mover ou alterar a ordem da janela. Esse ajuste acompanha o recorte nativo na criação, abertura, encaixe e movimento.

TDD: novo teste de remoção de título/botões falhou antes do helper e passou após a correção. 950 testes Rust, cargo fmt e Clippy passaram. Na janela real, antes e depois de dar foco, GetWindowLongPtr confirmou style=0x14000000, caption=false e nativeButtons=false. As pendências gerais da entrega permanecem abertas.

## Barra nativa durante o arrasto: correção complementar (2026-10-02)

O usuário confirmou que a barra reaparecia durante o arrasto. A verificação anterior de abertura/foco não demonstrava a correção desse gesto. Nesta investigação, um arrasto com mouse pressionado moveu a janela, mas manteve style=0x14000000; portanto, somente conferir os estilos não reproduziu nem descartou o defeito visual relatado.

O código da dependência Tao recalcula WS_CAPTION/WS_SYSMENU a partir dos flags internos em mudanças de estado. A proteção agora acompanha a vida do HWND: uma subclass impede a reinserção desses estilos em WM_STYLECHANGING, suprime WM_NCPAINT e encaminha WM_NCACTIVATE mantendo o tratamento de foco, com lParam=-1 para evitar a pintura nativa. A subclass é retirada em WM_NCDESTROY. O compositor também recebe DWMNCRP_DISABLED, que [desativa a renderização não cliente independentemente dos estilos](https://learn.microsoft.com/en-us/windows/win32/api/dwmapi/ne-dwmapi-dwmncrenderingpolicy). Isso preserva a transparência da WebView.

TDD: reinserir os estilos após a preparação falhou com a implementação anterior; agora passa. O teste também move a janela, envia mensagens de pintura/ativação e confirma DWMWA_NCRENDERING_ENABLED=false. **951 testes Rust passaram**, além de cargo fmt --check e Clippy --all-targets com warnings como erros. A primeira execução de Clippy encontrou DLL em uso pelo app; passou após encerrá-lo temporariamente, sem alterar os checks.

No app real atualizado, dois arrastos com mouse pressionado preservaram style=0x14000000 e a janela mudou de posição; com pin ativo, encaixou ao soltar. DWMWA_NCRENDERING_ENABLED=false foi confirmado. PrintWindow do HWND após o gesto: `docs/design/screens/assistant-native-drag-after.png`, sem barra duplicada. As tentativas de captura durante o gesto devolveram parcialmente a superfície da WebView e não servem como evidência visual do compositor durante todo o movimento; foram descartadas. Não foi feita captura da tela inteira. Verificação em outros monitores, cobertura mínima global e revisões finais continuam pendentes; sem commit de entrega completa.
