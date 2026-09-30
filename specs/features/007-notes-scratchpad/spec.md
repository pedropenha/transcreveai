# F007 — Notas por voz (Scratchpad)

**Status**: Draft · **Release**: v0.2 · **Depende de**: F002, F003, F010

## Contexto

Além de ditar no cursor, o usuário quer **tomar notas**: capturar ideias por voz que ficam guardadas no próprio app, sem precisar de um editor aberto. As notas de reunião (F009) usam o mesmo editor e armazenamento.

## Histórias

- **US-007-01** Quero apertar um atalho, falar uma ideia e ela ficar salva numa nota, sem mudar de janela.
- **US-007-02** Quero ver, buscar, editar e fixar minhas notas no Hub.
- **US-007-03** Quero copiar uma nota como Markdown para colar em outro app.

## Requisitos funcionais

### Captura
- **FR-007-01** Atalho "Nota por voz" (padrão `Ctrl+Win+Shift`, segurar; duplo toque para mãos-livres). Também acessível pelo 2º botão da Flow Bar antes da v0.3 e pelo menu ▾ depois (F001).
- **FR-007-02** O texto passa pelo pipeline (F004) com o perfil "Notas" (estilo `default`, limpeza no nível global) e é salvo como **nova nota** (`source = voice`); nada é inserido no app em foco.
- **FR-007-03** P2 — "Anexar à última nota se criada há menos de N min".
- **FR-007-04** Após salvar, toast não-ativável "Nota salva" com a primeira linha e botão "Abrir" (3 s).
- **FR-007-05** Título: primeira frase (≤ 60 chars); com LLM configurado, opção de gerar título curto.

### Scratchpad (Hub)
- **FR-007-06** Lista de notas ordenada por atualização, fixadas no topo; busca full-text (FTS5) com destaque.
- **FR-007-07** Editor Markdown (biblioteca escolhida via `search-first`) com salvamento automático (debounce 500 ms e ao fechar); ditar dentro do editor funciona como em qualquer campo (F005).
- **FR-007-08** Ações: nova nota, fixar, duplicar, excluir (com desfazer por 5 s), copiar como Markdown.
- **FR-007-09** P2 — "Organizar com IA" (tópicos/tarefas via LLM). Na v0.2, o usuário pode usar o Command Mode (F006) dentro do editor, como no Wispr.
- **FR-007-10** P2 — Exportação automática para uma pasta (`.md` com front-matter). Não existe no Wispr; na v0.2 basta "copiar como Markdown" (FR-007-08).
- **FR-007-11** P2 — Janela de captura rápida (atalho abre uma pequena janela focável para digitar/ditar uma nota).

## Critérios de aceitação

- **AC-007-01** *Dado* o navegador em foco, *quando* seguro `Ctrl+Win+Shift` e digo "comprar café", *então* nada é digitado no navegador, aparece o toast "Nota salva" e a nota "Comprar café." está no topo do Scratchpad.
- **AC-007-02** *Quando* busco "café" no Scratchpad, *então* a nota aparece com o termo destacado em ≤ 100 ms (com 10.000 notas).
- **AC-007-03** *Quando* uso "copiar como Markdown" numa nota e colo no Notion, *então* títulos e listas aparecem formatados. (A exportação para pasta é P2.)
- **AC-007-04** *Quando* fecho o Hub no meio de uma edição, *então* nada é perdido ao reabrir.

## Casos de borda

- Nota vazia (fala sem conteúdo após o pipeline) → não salvar; mostrar "Nada ouvido".
