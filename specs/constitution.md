# Constituição do Transcreve.ai

## 0. Hierarquia de autoridade

1. **ECC rules** (`~/.claude/rules/ecc/` — `common/` + `rust/`, `typescript/`, `react/`, `web/`) e **ECC skills/agentes** (plugin `ecc@ecc`) mandam em **tudo que é engenharia**: processo, TDD, cobertura, testes, revisão, estilo de código, tratamento de erros, segurança de código, git, design de frontend.
2. **Esta constituição e as specs** mandam no **produto**: o que o app faz, como se comporta para o usuário, escopo e prioridades.
3. Se uma spec contradizer uma rule ou skill em assunto de engenharia, **a rule vence** e a spec deve ser corrigida no mesmo PR.
4. As specs **não criam** exigências de engenharia próprias além das que as rules e skills já trazem; quando precisam de algo, apontam para a rule/skill correspondente.

## 1. Princípios de produto

### I. Paridade com o Wispr Flow
O comportamento de referência é o do **Wispr Flow** ([pesquisa](product/research-wispr-flow.md)). Na dúvida sobre como algo deve funcionar, faça como o Wispr faz. **Não** adicionar funcionalidades que o Wispr não tem, com uma única exceção deliberada: o princípio II.

### II. Transcrição local ou com chave própria
O usuário escolhe entre transcrever **localmente** (offline) ou via **API paga com a própria chave**. Esse é o único desvio intencional em relação ao Wispr (que é só nuvem e por assinatura) e tudo que ele exige (download de modelos, modo offline, fallback) faz parte do escopo.

### III. Privacidade
1. Com provedor local, nenhum áudio ou texto sai da máquina.
2. Envio para nuvem só para provedores configurados explicitamente pelo usuário.
3. Microfone aberto só durante sessão iniciada pelo usuário (ou auto-início que ele habilitou). Nada de gravação oculta.
4. Indicador visível sempre que houver gravação.
5. Sem conta, sem servidor próprio, sem telemetria.
6. Chaves de API num gerenciador de segredos (cofre do SO), conforme `rules/common/security.md` ("environment variables or a secret manager").

### IV. Nunca roubar o foco
Flow Bar e toasts são janelas não-ativáveis. O texto vai sempre para onde o cursor do usuário está.

### V. Rápido e sem perder a fala
Latência é requisito de produto (metas em [plan.md](architecture/plan.md#8-orçamentos-de-desempenho)). Sempre há caminho degradado (texto sem limpeza se o LLM falhar; fallback se a nuvem falhar) e a fala do usuário nunca é perdida por erro.

### VI. Windows primeiro
O produto roda em Windows, macOS e Linux. O MVP (v0.1–v0.3) é entregue e validado primeiro no Windows 11; o suporte multiplataforma herdado do Handy não é removido, só deixa de ser validado até o port (T-084).

### VII. Consentimento em reuniões
O app lembra o usuário de informar os participantes, nunca entra na chamada como bot e captura áudio só localmente.

### VIII. Idioma
Interface em pt-BR e en. O ditado nunca traduz o que foi dito, a menos que o usuário peça.

## 2. Engenharia

Governada inteiramente pelas ECC rules e skills (ver [plan.md §11](architecture/plan.md#11-engenharia-governada-pelas-ecc-rules)). Esta constituição não acrescenta regras de engenharia.

---

## Registro de alterações

| Data | Alteração | Motivo |
|---|---|---|
| 2026-09-30 | Versão inicial | — |
| 2026-09-30 | ECC rules/skills passam a prevalecer em engenharia; princípios de engenharia próprios removidos; adicionada paridade com o Wispr Flow (I) | Decisão do usuário após instalar o ECC |
