# ADR-0003: Identidade visual "Papel & Anil" e fontes self-hosted

**Date**: 2026-10-02
**Status**: accepted
**Deciders**: Pedro (produto), Claude (implementação)

## Context

A direção "Sinal Calmo" (T-008) usava cinzas esverdeados com texto de contraste baixo (`#707a71`, 4,4:1) e não dava hierarquia à UI. A proposta aprovada em `docs/design/proposta-ui.html` define uma nova identidade: canvas cor de papel, painel inset claro, anil como cor de ação, urucum reservado a gravação, tema escuro com o mesmo calor e overlays sempre escuros. Ela exige duas famílias tipográficas e funciona offline (F011).

## Decision

Adotamos a identidade **Papel & Anil** (tokens em `src/styles/theme.css`, claro e escuro, overlays `--ov-*`) e self-hospedamos as fontes **Instrument Sans** (variável) e **Instrument Serif** via `@fontsource`, importadas por todos os entrypoints (`src/styles/fonts.ts`). Os nomes de token antigos (`--color-*`, `--flowbar-*`, `--state-*`) permanecem como aliases. A barra de título nativa do Windows é mantida na v1.

## Alternatives Considered

### Alternativa 1: Google Fonts via CDN

- **Pros**: zero dependência no repositório.
- **Cons**: exige rede, quebra o modo offline, envia requisição a terceiros.
- **Why not**: viola a promessa de privacidade/offline (F011).

### Alternativa 2: Manter fontes do sistema (Segoe UI Variable)

- **Pros**: sem custo de bundle.
- **Cons**: sem caráter, sem serifada de destaque, aparência diferente por SO.
- **Why not**: a hierarquia tipográfica faz parte da identidade proposta.

### Alternativa 3: Titlebar própria (`decorations: false`)

- **Pros**: fidelidade total ao Flow.
- **Cons**: exige região de arraste, snap layouts do Windows 11 e testes por plataforma.
- **Why not**: custo/risco alto para ganho estético; reavaliar em ADR próprio se necessário.

## Consequences

### Extensão aprovada — Assistente Vidro & Anil (2026-10-02)

O usuário aprovou [a proposta do assistente](../design/proposta-assistente-vidro.html): vidro fosco, temas claro/escuro, compositor para teclado e voz e controles maiores. O assistente usa aliases `--as-*` ligados aos tokens semânticos do Hub, preservando a mesma paleta em claro/escuro; a exceção é somente de material translúcido e não de identidade cromática. Flow Bar/toasts mantêm `--ov-*`. Instrument Sans continua nos controles/texto, Serif nas boas-vindas. No Windows, Acrylic nativo foi removido porque pintava um retângulo nos cantos; a superfície CSS conserva translucidez sem blur nativo do desktop; a opção persistente Reduzir transparência oferece superfície sólida. Contraste e usabilidade prevalecem sobre a intensidade do efeito. O vidro não transforma o assistente em janela não ativável: clique explícito precisa permitir digitação, sem alterar a proteção de foco da Flow Bar.

### Positive

- Contraste WCAG 2.2 AA verificado por teste unitário (`src/styles/theme.test.ts`) nos dois temas e nos overlays.
- Fontes empacotadas pelo Vite, servidas da origem do app (~100 KB, `unicode-range`), compatíveis com `font-src 'self'`.
- Aliases evitam reescrever o CSS existente de uma vez.

### Negative

- Duas dependências novas (OFL 1.1; licença redistribuível no instalador).
- Raios e escala tipográfica mudam visualmente telas ainda não redesenhadas até as etapas seguintes.
- Valores do tema escuro aparecem duas vezes no CSS (`prefers-color-scheme` e `data-theme`); o teste garante que são idênticos.

### Risks

- `app.security.csp` está `null` em `tauri.conf.json` (sem CSP). As fontes não dependem disso; ao introduzir uma CSP, incluir `font-src 'self'`. Mitigação: registrado aqui para a tarefa de endurecimento de CSP.
- Nomes `--elev-*` foram usados em vez de `--shadow-*` da proposta para não colidir com o namespace de sombras do Tailwind.
