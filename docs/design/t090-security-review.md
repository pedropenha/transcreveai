# T-090 — revisão independente de segurança

Data: 2026-10-03. Escopo: providers CLI de F012, resolução nativa, configuração IPC/UI, seleção automática, limites e cancelamento. Revisão estática independente; execuções e cobertura devem ser conferidas no relatório de validação da tarefa.

## Resultado

Não restam achados concretos de severidade alta/crítica nas alterações revisadas. A aprovação de código não substitui aceitação autenticada nem cobertura. Cursor/Devin continuam experimentais: a seleção explícita no assistente permite execução sem garantia de modo não-mutante, conforme FR-012-01; não são autorizados em usos silenciosos ou seleção automática.

## Achados corrigidos durante a revisão

- Configuração herdada podia carregar hooks/MCP/customizações. `src-tauri/src/llm/cli_agent/adapters.rs` agora passa `--ignore-user-config`/`--ignore-rules` no Codex e `--safe-mode` no Claude. Os flags foram confrontados com o help instalado; Claude mantém autenticação, diferentemente de `--bare`.
- A lista de flags proibidos não cobria flags novas, subcomandos e clusters curtos. `validate.rs::validate_extra_args` aplica uma lista positiva de opções simples, tanto ao persistir quanto ao executar; não devolve o argumento arbitrário na mensagem de erro.
- `.cmd`/`.bat` eram executados por shell. `spawn.rs::native_launch_binary` usa esses arquivos somente para localizar o Codex nativo em caminhos fixos; não interpreta o shim. Scripts não entram em `run_headless`. Busca continua após candidato incompatível.
- `assistant/state.rs::choose_provider` devolvia experimental na alternativa final do modo Auto. O retorno agora exclui experimental; somente `assistant_provider_id` explícito permite essa seleção.
- `AssistantProvider` liberava experimental mesmo ausente/desabilitado. `assistantProviderOptions.ts` exige detecção e habilitação; a seleção separada de configuração pode acessar uma linha ausente para definir o override, sem selecionar o provider de execução.
- Cancelamento podia deixar tarefas de leitura/escrita destacadas. `PipeTaskGuard` aborta as tarefas ao descartar a chamada; `ChildTreeGuard` encerra a árvore antes de remover o diretório privado de trabalho.
- stderr podia conter prompt ou dados sensíveis mesmo após redação de tokens. A falha agora registra somente tamanho/status, sem conteúdo.

## Fronteiras verificadas

O ambiente usa lista positiva e remove API keys/COMSPEC. Prompt segue por stdin, sem arquivo ou argv. stdout/stderr têm limites. Timeout e Drop encerram a árvore. Overrides exigem caminho absoluto, arquivo executável, nome do adapter e rejeição UNC. Offline e habilitação são checados no roteador; o provider não exige chave nem grava credencial no keyring. O diretório de trabalho é privado e vazio; seu tempo de vida engloba o subprocesso.

A UI aguarda persistência e atualização de configurações/status, bloqueia seleção de configuração durante salvamento e apresenta falha; a mudança de provider de configuração não altera os providers do assistente/resumo. Não foi identificado um defeito concreto de concorrência nessa revisão estática.

## Limites

O executável instalado e a sessão autenticada do usuário são componentes confiados; validação de nome/caminho não autentica o fornecedor. Políticas administrativas do CLI ainda podem aplicar-se. Diretório vazio e sandbox somente leitura não equivalem a isolamento de leitura de todo o computador. Compatibilidade com versões diferentes dos CLIs exige repetir a aceitação dos flags; flags não reconhecidos falham sem remover as proteções. CSP global e alterações anteriores do assistente não foram reclassificadas como achados deste diff.

Validação comunicada pelo executor principal: os três testes autenticados opt-in passaram (resumo sintético Claude, resumo sintético Codex sem API key e cancelamento do processo Codex), em 19,37 s, pelo executável de testes compilado em um ambiente com LOCALAPPDATA real. Não foram enviados dados do histórico do usuário. Os testes focados backend comunicados passaram: 47 aprovados e três live inicialmente ignorados, posteriormente executados separadamente. Esta revisão não repetiu essas execuções.

Cobertura e aceitação UI continuam critérios separados, registrados no relatório final de validação da tarefa. O teste de cancelamento nativo verifica encerramento do subprocesso; não demonstra por si só a interação Esc/botão no painel.

A revisão complementar do manifesto aprovou a promoção de `tempfile = "3"` de dependência de teste para dependência normal, necessária ao diretório privado em `run_headless`. O crate já constava do lockfile; esta mudança não introduz nova versão nem implementação própria de gerenciamento de diretórios temporários. O resultado de Clippy e a auditoria de advisories devem ser registrados pelo executor principal.
