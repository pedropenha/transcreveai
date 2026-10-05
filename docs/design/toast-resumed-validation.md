# Validação retomada dos avisos flutuantes

Data: 2026-10-05. A implementação visual herdada do Claude já estava no checkout principal. Ela foi preservada: cartão dividido em aplicativo/texto/ações, botão e seta separados, elipse para nome longo e aviso de tradução com título e corpo completos.

Nesta retomada foram acrescentados testes de regressão e concluída a validação focada. Não foi necessário alterar o comportamento de produção dos avisos; os arquivos herdados em `src/toast/` receberam apenas formatação. A investigação recuperou o pedido original: melhorar proporções e espaçamento, preservar altura da janela, evitar mensagem cortada e confirmar acessibilidade.

## Evidências

- `tests/toast.spec.ts`: os 12 testes herdados passaram; oito novos verificam nome longo, controles separados e ordem de Tab nas escalas 100%, 125% e 150%, corpo integral do erro e altura reportada, fonte Instrument Sans, título em português e axe.
- Axe: nenhuma violação nos estados de detecção e erro de tradução, após a animação de entrada terminar. Não equivale a uma auditoria completa das interações nativas.
- Testes puros `toastView.test.ts` e `appIconCore.test.ts`: scripts de asserções passaram. Cobertura Bun: 100% de linhas e funções nesses dois módulos; não mede cobertura dos componentes React.
- ESLint de `src/toast/` e Prettier dos arquivos dessa frente passaram.
- Capturadas 48 imagens em `docs/design/screens/toast-resumed-*.png`, claro/escuro, 100%/150%, incluindo Meet, Zoom, Teams, aplicativo desconhecido com PNG, menu, gravação e avisos. Inspeção visual confirmou hierarquia, corpo integral, sombra dentro da margem e ações contidas.
- A inspeção encontrou um defeito no gerador de evidências herdado: redimensionar a página fechava o menu antes da captura. `tests/toast-screens.spec.ts` agora reabre e exige quatro itens visíveis; as quatro imagens do menu foram regeneradas e verificadas.

## Limites

Os testes usam Chromium e IPC Tauri simulado, com servidor isolado na porta 1437 e um worker. Não foi exercitada a janela nativa WebView2, foco entre aplicativos ou DPI real do Windows. Os gates gerais do repositório pertencem à validação conjunta da retomada; não são cobertos por este relatório focado. A duplicação do aviso de tradução no Hub foi encaminhada à frente responsável por `App.tsx`.

As imagens anteriores `toast-before-*` e `toast-after-*` são evidências herdadas e não foram removidas ou substituídas. Não foram feitos commits nesta frente.
