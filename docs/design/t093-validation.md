# T-093 — Implementação e validação

Data: 2026-10-03. Checkout: `C:\multimidia\ecc`, branch `integration/v1`. Alterações preexistentes do assistente e da prévia foram preservadas. Não houve commit nesta execução.

## Comportamento implementado

Configurações → Transcrição → Idiomas oferece modelo local próprio para o ditado traduzido e download/cancelamento explícitos. O atalho `transcribe_translate` também aparece em Uso → Atalhos: padrão `Ctrl+Alt+Space` no Windows/Linux, `Option+Cmd+Space` no macOS, configurável. O usuário aciona o atalho no aplicativo de destino; não há botão que inicie captura pelo Hub.

Apenas a sessão iniciada por essa ação traduz para inglês. Preferências globais, modelo de ditado e provedores permanecem intactos. A capacidade do catálogo e do motor efetivamente carregado é validada; Turbo e motores incompatíveis não produzem falsa tradução. O texto traduzido passa pelo pipeline determinístico como inglês, sem LLM nem fallback para nuvem. A Flow Bar identifica o modo de tradução.

O motor compartilhado impede começar uma tradução durante gravação de reunião ou carga de outro modelo. Erros são localizados em pt-BR/en e apresentados também pelo toast nativo não ativante, mesmo com o Hub oculto; códigos desconhecidos não expõem detalhes técnicos. Cancelamento e mudança de janela no Windows bloqueiam a entrega também depois do atraso de colagem e da espera por modificadores. O caminho protegido restaura o clipboard ao abortar; scripts externos são recusados antes da gravação nesse modo. O caminho comum de inserção mantém seu comportamento.

## Verificações concluídas

| Verificação                          | Resultado                                                                          | Alcance                                                                                                                      |
| ------------------------------------ | ---------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------- |
| `bun run build`                      | PASS                                                                               | TypeScript + bundle Vite de produção; não é empacotamento Tauri                                                              |
| `bun run test:unit`                  | PASS                                                                               | Suite configurada de unidades do frontend, incluindo seletor de tradução                                                     |
| `bun run test:coverage`              | PASS, 22 testes                                                                    | Runner de cobertura unitária; não comprova 80% em todos os módulos herdados                                                  |
| `bun run check:translations`         | PASS                                                                               | pt-BR/en completos                                                                                                           |
| `bun run lint`                       | PASS, zero erros e 15 avisos existentes                                            | Frontend inteiro                                                                                                             |
| Playwright completo                  | PASS, 167 aprovados e 36 não executados, antes do último ajuste de aviso flutuante | Chromium com IPC Tauri simulado; capturas visuais exigem `CAPTURE_SCREENS=1`                                                 |
| Playwright final das áreas alteradas | PASS, 56 testes                                                                    | T-093, toast, Flow Bar, Configurações e overlays após o aviso flutuante                                                      |
| Playwright T-093 final               | PASS, 12 casos                                                                     | Escolha independente, teclado pt-BR, download/cancelamento, erros, rollback, modo da Flow Bar e toast pt-BR/en sem focar Hub |
| Axe no card de tradução              | Zero violações                                                                     | Acessibilidade automatizada; não substitui leitor de tela manual                                                             |
| Cobertura V8 do card e helper        | 100% linhas/funções/branches/statements                                            | `TranslatedDictationSettings.tsx` e `translationModels.ts`, com sourcemaps e hashes de fonte                                 |
| Revisão independente                 | Sem CRITICAL/HIGH na revisão do código integrado                                   | Isolamento, carregamento, privacidade, destino/cancelamento e clipboard; revisão não executa aceitação nativa                |

Backend final:

| Verificação                                 | Resultado                                            | Alcance                                                                                                                                                                  |
| ------------------------------------------- | ---------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `cargo test --lib`                          | PASS, 972 aprovados e 3 ignorados                    | Inclui comparação literal dos bindings gerados                                                                                                                           |
| Testes Rust focados da T-093                | PASS, 17 testes                                      | Isolamento das opções pt/en, capacidade/catálogo, identidade do modelo carregado, carga simultânea, coordenador/Flow Bar, cancelamento e política de entrega/restauração |
| `cargo clippy --lib --tests -- -D warnings` | PASS                                                 | Biblioteca e testes Rust                                                                                                                                                 |
| `cargo fmt --check`                         | PASS                                                 | Backend                                                                                                                                                                  |
| LLVM `translation.rs`                       | 100%: 132/132 linhas, 153/153 regiões, 12/12 funções | Módulo de políticas incluindo seus testes; não é cobertura de todo o caminho FFI/inserção                                                                                |

A detecção do idioma de origem é automática somente no snapshot traduzido, inclusive quando o idioma global é inglês. Uma gravação comum pode capturar enquanto a carga do mesmo modelo termina, mantendo o comportamento anterior; uma carga de seleção diferente é recusada. A revisão independente foi repetida após esses ajustes e terminou sem CRITICAL/HIGH.

Formatação dos arquivos de frontend/documentação alterados passou. `git diff --check` do backend passou; no diff completo permanecem dois espaços finais em `src/bindings.ts` produzidos pelo exporter. Eles foram preservados para manter igualdade literal com a geração, verificada por teste. A catraca de cobertura Rust global `--workspace --all-targets` não foi medida nesta execução; a cobertura isolada não a substitui.

## Limites e aceitação nativa pendente

Não foi comprovada nesta entrega a tradução de fala real e colagem em Bloco de Notas/Chrome/VS Code, nem a restauração de texto/imagem real do clipboard sob cancelamento ou troca de HWND. Os WAVs existentes do harness são sinais sintéticos, sem fala inteligível, e não validam a qualidade de tradução. Não houve empacotamento NSIS, teste de instalação ou aceite em macOS/Linux.

O histórico preserva áudio/texto conforme as políticas existentes, mas a intenção de tradução por sessão não é um contrato de retry persistido; o botão genérico "Tentar novamente" não deve ser interpretado como garantia de repetir a tradução. Scripts externos e reliable paste com confirmação via UI Automation não são usados no caminho traduzido protegido. A validação nativa deve conferir essas diferenças e os métodos efetivamente configurados.

Versão de lançamento e acionamento por voz permanecem em aberto. A T-093 conserva seu checkbox aberto até a aceitação nativa; isso não indica ausência do código implementado.

Checklist de aceitação: configurar/baixar um modelo compatível; ditar em português pelo novo atalho e conferir inglês no destino; ditar pelo atalho comum e conferir idioma/modelo originais; repetir com cancelamento e mudança de janela antes/durante o atraso de colagem; conferir restauração do clipboard; iniciar reunião e confirmar que tradução é rejeitada sem afetá-la; testar modelo não baixado/incompatível e o botão de parar da Flow Bar.

Planejamento e pesquisa: [t093-orchestration.md](t093-orchestration.md).
