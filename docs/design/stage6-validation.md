# Papel & Anil — etapa 6

Data: 2026-10-02. Checkout: `C:\multimidia\ecc`, branch `integration/v1`.
Referência: `proposta-ui.html`, abas Dicionário, Componentes e Escopo & plano;
ADR-0003. Relatório de frontend; não representa aceitação nativa do release.

## Implementação

- Dicionário em Vocabulário, Substituições e Muletas, com contagens, busca,
  adição/remoção, restauração de muletas e prévia antes/depois editável.
- Vocabulário/muletas usam os comandos existentes. O estado só confirma uma
  escrita bem-sucedida; falhas mantêm rascunho/lista, e leitura com erro oferece
  nova tentativa. Operações pendentes impedem edições concorrentes.
- Substituições permanecem indisponíveis na v1. A prévia é aproximada, destaca
  vocabulário e ilustra muletas; não simula STT, substituições nem todas as
  condições do pipeline. Aprendizado automático, filtros de origem aprendida e
  edição individual da proposta dependem de suporte futuro (FR-010-28).
- Flow Bar e toast usam a paleta `--ov-*`; o assistente já usa seus aliases.
  Testes verificam cores resolvidas e Instrument Sans nos três entrypoints.
  Instrument Serif carrega e é aplicada ao wordmark e aos destaques do Hub.
- Regiões principais/rodapé, nomes de controles e contraste corrigidos nos
  componentes compartilhados. Regras de hooks adicionadas ao ESLint.
- Textos novos em inglês e pt-BR; nenhum `console.log` novo.

## Verificação

- `bunx tsc --noEmit`: PASS.
- `bun run lint`: PASS, zero erros; 15 avisos de dependências de hooks em
  trechos preexistentes, expostos pela nova configuração. Não foram silenciados.
- `bun run test:unit`: PASS, 23 módulos de testes com assertions.
- `bun run test:coverage`: PASS, 13 cenários do agregador; helper novo
  `dictionaryView.ts` com 100% de funções/linhas.
- `bun run check:translations`: PASS, todas as chaves de en presentes em pt-BR.
- `bunx prettier --check src tests docs specs`: PASS.
- Suíte completa Playwright com capturas: 178 testes PASS, sem skips.
  Inclui 34 cenários axe (17 telas/estados em cada tema), sem violações de
  WCAG 2.2 AA/boas práticas, sem regras desativadas nem elementos excluídos;
  mais quatro testes de teclado/carregamento de fontes.
- ECC `react-reviewer` e `typescript-reviewer`: nenhum CRITICAL/HIGH residual.
- Cobertura real Chromium remapeada para TS/TSX: todos os três arquivos do
  Dicionário superam 80% em linhas, funções, branches e statements.
  A medição falha com fontes ausentes/desatualizadas ou sem execução de funções.

| Arquivo                | Linhas | Funções | Branches |
| ---------------------- | ------ | ------- | -------- |
| DictionarySettings.tsx | 98,92% | 100%    | 94,62%   |
| DictionaryPreview.tsx  | 100%   | 100%    | 85,71%   |
| dictionaryView.ts      | 98,87% | 100%    | 87,50%   |

Para repetir a medição, no PowerShell, usando um ID de execução novo:

```powershell
$env:DICTIONARY_COVERAGE = '1'
$env:DICTIONARY_COVERAGE_RUN_ID = 'stage6-recheck'
bunx playwright test tests/dictionary.spec.ts
bun scripts/check-dictionary-coverage.ts
```

Os relatórios agregados ficam em `coverage/dictionary/<ID>/coverage-final.json`
(ignorados pelo Git). A coleta só fica ativa com `DICTIONARY_COVERAGE=1`.

## Prints

36 arquivos em `docs/design/screens/`, prefixo `stage6`, 900/1280/1600 px,
claro/escuro. Capturas do viewport do frontend Chromium, sem área de trabalho
nem conteúdo de outros aplicativos. Inspeção visual do Dicionário confirma
coluna da prévia à direita em largura ampla e abaixo da lista em painel estreito.
São referências para comparação humana, não baselines de comparação automática.

| Tela                 | Padrão de arquivo                                |
| -------------------- | ------------------------------------------------ |
| Início               | `stage6-{light,dark}-{900,1280,1600}.png`        |
| Notetaker            | `stage6-notetaker-{tema}-{largura}.png`          |
| Modelos              | `stage6-settings-models-{tema}-{largura}.png`    |
| Dicionário, 3 grupos | `stage6-dictionary-{grupo}-{tema}-{largura}.png` |

Grupo: `vocabulary`, `replacements`, `crutches`. Para repetir:

```powershell
$env:CAPTURE_SCREENS = '1'
$env:SCREEN_PREFIX = 'stage6'
bunx playwright test tests/visual.spec.ts
```

Exemplos: [Vocabulário claro 1280](screens/stage6-dictionary-vocabulary-light-1280.png),
[Muletas escuro 900](screens/stage6-dictionary-crutches-dark-900.png).

## Limites e pendências

- Nenhum Rust alterado; `cargo test`/`clippy` não executados nesta etapa.
- Não havia processo ou janela `Transcreve.ai` disponível na inspeção. Não foi
  iniciado `tauri dev`, nem capturada a tela inteira. Os testes usam IPC Tauri
  simulado: não confirmam persistência em disco, gravação, ditado/colagem,
  foco/topmost das janelas nativas ou empacotamento. Smoke nativo segue pendente.
- Axe e teclado não substituem auditoria manual com leitor de tela. Latência
  também não foi medida; T-085 continua aberta.
- T-044 permanece aberta para aceitação funcional de todos os controles,
  inclusive microfone/atalhos no app real. O redesign das telas está implementado.
- `.tmp-consent.png` e `session-report.html` preservados, fora do commit.
