# F011 — Segurança e privacidade

**Status**: Draft · **Release**: MVP · **Transversal** (vale para todas as features)

## Contexto

O app escuta o microfone, instala um hook global de teclado, injeta teclas, lê o clipboard e guarda chaves de API pagas. Isso exige cuidado explícito — tanto para proteger o usuário quanto para não ser tratado como malware.

## Requisitos funcionais

### Segredos
- **FR-011-01** Chaves de API são gravadas **somente** no cofre do SO — o "secret manager" de `rules/common/security.md` (Windows Credential Manager, entrada `Transcreve.ai/provider/<provider_id>`; macOS Keychain). Variáveis de ambiente só em testes/CI.
- **FR-011-02** O frontend só **escreve** chaves (`secret_set`); não existe comando para lê-las. A UI mostra `••••` + últimos 4 caracteres (`secret_hint`).
- **FR-011-03** Chaves nunca aparecem em: logs (redação automática de padrões `sk-…`, `gsk_…`, `Bearer …`, `x-api-key`), mensagens de erro, relatórios de crash, exportação de configurações, SQLite.
- **FR-011-04** Excluir um provedor apaga a entrada do cofre.
- **FR-011-05** Validação de formato por provedor antes de salvar (aviso, não bloqueio) + "Testar conexão".

### Rede
- **FR-011-06** Somente HTTPS, exceto `http://` para `localhost`, `127.0.0.1`, `::1` (Ollama, servidores locais).
- **FR-011-07** O app só se conecta a: hosts dos provedores configurados, URLs de download do catálogo de modelos e o servidor de atualização. Nenhuma outra conexão de saída (verificável).
- **FR-011-08** **Modo offline** (Configurações e bandeja): bloqueia toda chamada de rede de provedores; a UI indica claramente; provedores em nuvem ficam indisponíveis e o fallback local é usado.
- **FR-011-09** Sem telemetria/analytics na v1. Relatório de erro só é gerado localmente e o usuário decide enviar (P2).
- **FR-011-10** Respeitar proxy do sistema / `HTTPS_PROXY`.

### O que sai da máquina (transparência)
- **FR-011-11** Tela "O que é enviado" em Privacidade, gerada a partir da configuração atual:
  | Dado | Para quem | Quando |
  |---|---|---|
  | Áudio do ditado | Provedor STT de ditado (se nuvem) | A cada ditado |
  | Texto ditado + categoria/nome do app | Provedor LLM (se configurado) | Limpeza (F004) |
  | Texto selecionado + instrução | Provedor LLM | Command Mode (F006) |
  | Áudio da reunião | Provedor STT de reuniões (se nuvem) | Gravação/processamento |
  | Transcrição + minhas notas | Provedor LLM | Resumo (F009) |
  | Título da janela | Provedor LLM | Só se `send_window_title` = ligado |

### Dados locais
- **FR-011-12** Retenção configurável (ver [data-model](../../architecture/data-model.md#4-retenção)); limpeza automática.
- **FR-011-13** "Apagar todos os dados": remove banco, áudios, notas, modelos (opcional) e segredos, com confirmação digitada.
- **FR-011-14** P2: criptografia do banco (SQLCipher) com chave no cofre do SO.
- **FR-011-15** Arquivos criados com permissões apenas do usuário (pasta em `%APPDATA%`, herdando ACL do perfil).

### Gravação e consentimento
- **FR-011-16** Nunca gravar sem sessão explícita (ou auto-início habilitado pelo usuário). Se o "microfone aquecido" (P2, FR-002-10) vier a existir, o pré-roll fica só em memória e é descartado continuamente.
- **FR-011-17** Indicador de gravação sempre visível (Flow Bar + bandeja) — não configurável.
- **FR-011-18** Aviso de consentimento para reuniões (F009 FR-009-02).

### Hook de teclado e injeção
- **FR-011-19** O hook só compara eventos com as combinações configuradas; **não** registra, armazena ou transmite teclas. Mudanças em `hotkeys/`, `secrets/`, `insertion/` e nos provedores de rede disparam os gatilhos de segurança de `rules/common/code-review.md` (`ecc:security-reviewer`).
- **FR-011-20** Eventos injetados são marcados e ignorados pelo próprio hook.
- **FR-011-21** O app roda **sem** privilégios de administrador; não oferece "executar como admin".

### LLM e injeção de prompt
- **FR-011-22** Chamadas de LLM **sem ferramentas**; a saída é sempre tratada como texto a inserir, nunca como comando.
- **FR-011-23** Conteúdo do usuário (ditado, seleção, transcrição) sempre delimitado e declarado como dado no prompt; salvaguardas de saída da F004 (FR-004-15).

### Cadeia de suprimentos
- **FR-011-24** Modelos baixados verificados por SHA-256 fixado no catálogo; download só de URLs do catálogo ou de arquivo importado pelo usuário.
- **FR-011-25** Instalador e binários assinados; atualizações assinadas (chave do updater do Tauri).
- **FR-011-26** Auditoria de dependências conforme `rules/rust/security.md` (`cargo audit`, `cargo deny check`) e equivalente para o frontend; lockfiles versionados.
- **FR-011-27** Nenhum servidor HTTP/WebSocket local exposto na v1 — por isso os itens de `rules/common/security.md` sobre endpoints (CSRF, rate limiting, autenticação) não se aplicam hoje. Se um dia existir um (ex.: MCP, como o Wispr tem), esses itens passam a valer integralmente.

## Critérios de aceitação

- **AC-011-01** *Dado* uma chave salva, *quando* procuro o valor da chave em todo `%APPDATA%\br.com.creator4all.transcreve.ai` e `%LOCALAPPDATA%\br.com.creator4all.transcreve.ai` (banco, logs, settings), *então* não encontro nenhuma ocorrência.
- **AC-011-02** *Dado* o modo offline e provedor em nuvem selecionado, *quando* dito, *então* nenhuma conexão sai da máquina (monitor de rede) e o ditado usa o local ou mostra erro claro.
- **AC-011-03** *Dado* o modo debug desligado, *então* os logs não contêm texto transcrito, apenas metadados (tamanhos, latências, códigos de erro).
- **AC-011-04** *Quando* uso "Apagar todos os dados", *então* banco, áudios, notas e entradas do cofre são removidos.
- **AC-011-05** *Quando* altero o SHA-256 de um modelo no catálogo de teste, *então* o download é rejeitado.
- **AC-011-06** *Dado* `base_url = http://api.exemplo.com`, *então* a configuração é rejeitada; `http://localhost:11434/v1` é aceito.

## Notas legais (não é aconselhamento jurídico)

- Gravação de conversas: informar os participantes é boa prática e pode ser exigência legal/contratual; a LGPD trata voz e transcrições como dados pessoais. O app facilita o aviso, mas a responsabilidade é do usuário.
- Termos dos provedores de API (retenção de dados, uso para treino) variam; a tela de provedores deve linkar a política de cada um.
