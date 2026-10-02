# Direção de design — Transcreve.ai

**Status:** aprovado para implementação incremental · **Tarefa:** T-008 · **Direção:** ~~Sinal Calmo~~ substituída por Papel & Anil ([ADR-0003](docs/adr/0003-identidade-visual-papel-e-anil.md); tokens vigentes em `src/styles/theme.css`)

Este documento orienta a implementação visual do Hub, da Flow Bar, dos toasts e dos estados de gravação. Ele não implementa as telas previstas em T-040 e T-042. O contrato legível por máquina está em [`design-tokens.json`](design-tokens.json), e a referência visual autocontida está em [`design-preview.html`](design-preview.html).

## 1. Direção escolhida

**Sinal Calmo** combina a discrição de um instrumento de sistema com sinais claros de atividade. O produto deve desaparecer quando está ocioso e tornar estado, progresso e próxima ação imediatamente reconhecíveis quando entra em uso.

A ergonomia, a densidade e o polimento observados no Wispr Flow são referências de comportamento. A identidade não é copiada: o Transcreve.ai usa uma paleta própria, tipografia nativa, iconografia Lucide já adotada pelo projeto e uma linguagem baseada em **trilhos de sinal**, não nos assets, nas cores ou na marca do Wispr.

### Por que esta direção

- **Uso repetido:** controles compactos, leitura rápida e pouco ruído visual reduzem fadiga.
- **Contexto desktop:** o sistema precisa permanecer legível sobre qualquer aplicativo sem parecer uma janela promocional.
- **Privacidade e confiança:** estados de captura são explícitos, persistentes e não dependem só de cor.
- **Produto local-first:** superfícies sólidas e precisas comunicam ferramenta local, não serviço web genérico.
- **Escala:** os mesmos tokens atendem Hub, Flow Bar, toast, reunião, onboarding e configurações em light/dark.

## 2. Princípios visuais

### 2.1 Ocioso quase invisível; ativo inequívoco

A Flow Bar em `idle` é apenas uma fenda escura de 48 × 8 px. Ao gravar, o aumento de área, a forma de onda, o texto acessível e o sinal coral deixam a captura inequívoca. Não existe animação decorativa em repouso.

### 2.2 Uma superfície de trabalho, não um mosaico

O Hub usa uma área contínua com divisores, listas e painéis de detalhe. Cartões só são aceitáveis quando representam objetos independentes; não devem embrulhar cada configuração ou cada métrica.

### 2.3 Cor tem função estável

- **Teal de sinal:** ação primária, foco e escuta pronta.
- **Coral de captura:** gravação de ditado ou reunião.
- **Violeta de comando:** modo de transformação/comando.
- **Âmbar:** processamento, atenção e reunião detectada.
- **Verde:** conclusão e sucesso.
- **Vermelho:** falha que exige correção.

Gravação e erro não compartilham o mesmo token. Todo estado combina cor com ícone, rótulo, forma ou movimento.

### 2.4 Densidade respirável

A unidade espacial é 4 px. Controles recorrentes usam 40 px de altura e áreas de alvo preferenciais de 44 × 44 px. A Flow Bar é a exceção visual compacta, mas seus alvos interativos devem ampliar a área clicável sem alterar a silhueta.

### 2.5 Profundidade por necessidade

Bordas separam regiões; sombras indicam flutuação. O Hub usa elevação mínima. Flow Bar, menus, tooltips, toasts e diálogos recebem sombras em duas camadas. Não usar glassmorphism, gradientes ornamentais ou cartões aninhados.

## 3. Linguagem distintiva: trilho de sinal

O detalhe memorável é um **trilho de sinal** fino que conecta navegação, histórico e estado:

- no Hub, aparece como linha de 2 px junto à seleção atual e como eixo temporal do histórico;
- em listas, pontos no trilho distinguem ditado, comando, nota e reunião;
- na Flow Bar, torna-se a forma de onda central;
- em toasts, aparece como ponto de estado acompanhado de texto;
- em progresso, pode preencher horizontalmente sem adicionar uma barra pesada.

O trilho nunca é puramente decorativo: ele sempre codifica seleção, tempo, nível ou estado.

## 4. Paleta light/dark

Os valores brutos vivem apenas em `primitive.color` de `design-tokens.json` e nas propriedades primitivas equivalentes de `src/styles/theme.css`. Componentes consomem tokens semânticos.

| Papel              | Light                         | Dark                         | Uso                         |
| ------------------ | ----------------------------- | ---------------------------- | --------------------------- |
| Canvas             | `semantic.color.light.canvas` | `semantic.color.dark.canvas` | Fundo do Hub                |
| Superfície         | `…surface`                    | `…surface`                   | Área de leitura e controles |
| Superfície elevada | `…surfaceRaised`              | `…surfaceRaised`             | Menus, dialogs, toasts      |
| Texto              | `…text`                       | `…text`                      | Conteúdo principal          |
| Texto secundário   | `…textMuted`                  | `…textMuted`                 | Metadados e descrições      |
| Borda              | `…border`                     | `…border`                    | Separação de regiões        |
| Acento             | `…accent`                     | `…accent`                    | Ação, seleção e escuta      |
| Gravação           | `…recording`                  | `…recording`                 | Captura ativa               |
| Processamento      | `…processing`                 | `…processing`                | Trabalho em andamento       |
| Sucesso            | `…success`                    | `…success`                   | Conclusão                   |
| Aviso              | `…warning`                    | `…warning`                   | Atenção e reunião detectada |
| Erro               | `…error`                      | `…error`                     | Falha acionável             |
| Comando            | `…command`                    | `…command`                   | Ditado em modo comando      |

A Flow Bar usa `semantic.component.flowBar.light|dark`: em ambos os temas ela permanece grafite para manter contraste previsível sobre janelas externas. O tema altera sutilezas de superfície, borda e luminosidade do sinal, não a identidade do instrumento.

## 5. Tipografia

### Famílias

- **Interface:** `typography.family.sans` — Segoe UI Variable/Segoe UI no Windows, com fallbacks nativos. Não requer fonte externa.
- **Tempos e dados:** `typography.family.mono` — Cascadia Mono, SFMono-Regular ou Consolas.

### Escala

| Uso                    | Token      | Peso    | Entrelinha           |
| ---------------------- | ---------- | ------- | -------------------- |
| Rótulo mínimo/atalho   | `size.xxs` | 500–600 | `lineHeight.compact` |
| Metadado/caption       | `size.xs`  | 400–600 | `lineHeight.snug`    |
| Controle e lista densa | `size.sm`  | 500     | `lineHeight.snug`    |
| Corpo padrão           | `size.md`  | 400     | `lineHeight.body`    |
| Título de seção        | `size.lg`  | 600     | `lineHeight.snug`    |
| Título de página       | `size.2xl` | 650/700 | `lineHeight.compact` |
| Número de destaque     | `size.3xl` | 600     | `lineHeight.compact` |

Títulos curtos usam `text-wrap: balance`; descrições curtas usam `text-wrap: pretty`; cronômetros e métricas usam algarismos tabulares.

## 6. Espaçamento, raios, bordas e elevação

### Espaçamento

A escala vai de `space.0` a `space.16`, com 4 px como unidade principal. Combinações recomendadas:

- ícone + rótulo: `space.2`;
- controles no mesmo grupo: `space.3`;
- padding de linha densa: `space.3` × `space.4`;
- padding de painel: `space.5` ou `space.6`;
- separação de seções: `space.8`;
- margem de página: `space.8` a `space.12`, responsiva.

### Raios concêntricos

- controles pequenos: `radius.sm`;
- campos e linhas agrupadas: `radius.md`;
- menus/toasts: `radius.lg`;
- dialogs e painéis flutuantes: `radius.xl`;
- Flow Bar, badges e toggles: `radius.pill`.

Ao aninhar superfícies, o raio externo deve equivaler aproximadamente ao raio interno mais o padding. Não arredondar divisores, tabelas ou áreas contínuas sem necessidade.

### Bordas

- padrão: 1 px com `color.border`;
- ênfase: 1 px com `color.borderStrong`;
- foco: anel de 2 px com offset de 2 px;
- gravação/erro: borda de estado mais ícone/rótulo, nunca cor isolada.

### Sombras

- `shadow.raised`: menu, seletor e tooltip;
- `shadow.floating`: Flow Bar e toast;
- `shadow.modal`: diálogo;
- nenhum shadow em linhas do histórico ou grupos de configurações.

## 7. Movimento

| Situação           | Duração                       | Easing                   | Regra                            |
| ------------------ | ----------------------------- | ------------------------ | -------------------------------- |
| Pressão            | `motion.duration.fast`        | `motion.easing.standard` | escala máxima de 0,98            |
| Hover/foco         | `motion.duration.interaction` | `motion.easing.standard` | cor, borda e sombra explícitas   |
| Entrada            | `motion.duration.enter`       | `motion.easing.enter`    | opacidade + 4 px de deslocamento |
| Saída              | `motion.duration.interaction` | `motion.easing.exit`     | mais curta e silenciosa          |
| Mudança de estado  | `motion.duration.state`       | `motion.easing.enter`    | largura/forma/opacidade          |
| Confirmação `done` | `motion.duration.success`     | —                        | permanência total de 600 ms      |

Não usar `transition: all`. Animações de onda atuam apenas em `transform`/`opacity`; o nível de áudio não deve provocar layout global. Em `prefers-reduced-motion: reduce`, durações tornam-se instantâneas, pulsos param e os estados trocam sem escala ou deslocamento.

## 8. Estados da Flow Bar

| Estado da F001      | Superfície              | Sinal              | Forma e conteúdo                                          | Movimento               |
| ------------------- | ----------------------- | ------------------ | --------------------------------------------------------- | ----------------------- |
| `hidden`            | —                       | —                  | nada visível                                              | nenhum                  |
| `idle`              | `flowBar.surface`       | `state.default`    | fenda 48 × 8 px, borda sutil                              | nenhum                  |
| `hover`             | `flowBar.surfaceRaised` | `state.hover`      | cápsula 88 × 36 px, duas ações com área de alvo ampliada  | entrada ≤ 150 ms        |
| `recording`         | `flowBar.surfaceRaised` | `state.recording`  | 128 × 36 px, cancelar, 7 barras, parar e rótulo acessível | barras respondem ao mic |
| `recording_command` | `flowBar.surfaceRaised` | `state.command`    | 150 × 36 px, ícone/rótulo “Comando”                       | igual a gravação        |
| `processing`        | `flowBar.surface`       | `state.processing` | 64 × 24 px, três pontos + texto para AT                   | pulso discreto          |
| `done`              | `flowBar.surface`       | `state.success`    | 48 × 24 px, check + anúncio                               | 600 ms e volta a idle   |
| `error`             | `flowBar.surface`       | `state.error`      | 64 × 24 px, ícone, borda e mensagem no hover              | sem pulso               |
| `meeting_recording` | `flowBar.surfaceRaised` | `state.recording`  | 96 × 24 px, ponto, “Gravando” e cronômetro mono           | pulso reduzível         |

A forma de onda visual pode ter 7 barras conforme F001. A implementação herdada ainda usa 9 barras; a alteração funcional/estrutural pertence à T-040 e não é feita aqui.

## 9. Toast de reunião e pílula de reunião

### Toast F008

- compacto: ícone, “Reunião detectada”, app e “Agora” em uma linha de leitura de 44–52 px;
- expandido: mesma superfície cresce verticalmente, sem trocar de posição; fechar, ação primária e menu mantêm alvos de 40/44 px;
- sinal âmbar + ícone de câmera + texto evitam significado apenas por cor;
- ancorado no slot acima da Flow Bar com `zIndex.toast`, sem sobreposição;
- confirmação de início troca para coral e texto “Gravando · <App>”.

### Pílula F009

- coral persistente, cronômetro mono e verbo “Gravando”;
- Pausar e Parar têm ícones e nomes acessíveis;
- nunca pode ser visualmente confundida com `done` ou `error`;
- fechamento da janela de reunião não altera a pílula.

## 10. Direção do Hub

A organização funcional da F010 é mantida, mas a expressão visual evita “sidebar + cards”.

### Estrutura

1. **Trilho de navegação compacto:** 56–64 px recolhido e até 176 px quando houver espaço; seleção marcada pela linha de sinal e por contraste, não por um cartão arredondado.
2. **Faixa de contexto:** título, busca/ações e status local/offline em uma faixa única, sem hero.
3. **Área contínua:** histórico em lista temporal com divisores; estatísticas em uma faixa tipográfica, não em quatro cartões.
4. **Painel de detalhe:** abre ao lado em larguras amplas ou como folha em largura estreita; mantém a lista visível quando possível.
5. **Configurações:** seções com cabeçalhos e linhas divididas; grupos podem ter uma única borda externa, sem card por setting.

### Densidade e responsividade

- largura confortável de leitura: 680–760 px;
- linha de histórico: mínimo de 64 px, texto em até duas linhas;
- metadados secundários alinhados, nunca competindo com o texto transcrito;
- abaixo de 760 px, o trilho vira uma faixa superior/rodapé de navegação e o detalhe ocupa a área;
- 400% de zoom deve preservar conteúdo e ações sem rolagem horizontal bidimensional.

## 11. Aplicação em componentes existentes

| Componente                                | Aplicação futura dos tokens                                                            |
| ----------------------------------------- | -------------------------------------------------------------------------------------- |
| `Button`                                  | `state.default/hover/active/focus/disabled`, altura confortável e foco de 2 px         |
| `Input`, `Textarea`, `Select`, `Dropdown` | `surfaceInset`, `border`, `borderStrong`, `focusRing`; sem `transition: all`           |
| `SettingsGroup`                           | uma superfície contínua com divisores; remover aparência de cartões repetidos na T-044 |
| `Dialog`                                  | `surfaceRaised`, `shadow.modal`, `radius.xl`, backdrop sem cor de marca                |
| `Tooltip`                                 | `flowBar.surfaceRaised` ou `surfaceRaised`, sombra `raised`, texto conciso             |
| `Alert`, `Badge`                          | tokens semânticos de sucesso/aviso/erro em vez de cores Tailwind soltas                |
| `AudioPlayer`, `Slider`                   | trilho semântico, thumb com foco e área de alvo ampliada                               |
| `RecordingOverlay`                        | aliases `--flowbar-*` ligados aos estados, sem reimplementar a máquina da F001         |

Os aliases legados (`--color-background-ui`, `--color-logo-primary`, `--color-mid-gray` etc.) permanecem durante a migração para evitar uma troca ampla fora da T-008. Novos componentes devem preferir nomes semânticos.

## 12. Acessibilidade

- contraste mínimo WCAG AA: 4,5:1 para texto normal e 3:1 para texto grande, ícones essenciais e limites de controles;
- estados de foco visíveis com anel de 2 px e offset de 2 px nos dois temas;
- área interativa mínima WCAG 2.2 de 24 × 24 px; alvo recomendado de 44 × 44 px e mínimo prático de 40 × 40 px no Hub;
- botões somente com ícone recebem nome acessível; ícones decorativos usam `aria-hidden`;
- mudanças `recording`, `processing`, `done` e `error` devem ser anunciadas em região `aria-live="polite"`, sem anunciar cada amostra do microfone;
- formas de onda e pulsos não carregam informação sozinhos; há texto/ícone equivalente;
- cronômetros usam algarismos tabulares para evitar deslocamento;
- `prefers-reduced-motion` elimina pulso, shimmer, escala e deslocamento;
- alvos compactos da Flow Bar usam hit area expandida sem sobreposição;
- tema explícito `data-theme="light|dark"` prevalece sobre `prefers-color-scheme`; sem atributo, o sistema decide.

## 13. Governança dos tokens

1. `design-tokens.json` é o contrato canônico legível por máquina.
2. `src/styles/theme.css` é o espelho de runtime em CSS custom properties.
3. `src/App.css` registra no Tailwind 4 apenas tokens consumidos por utilities.
4. `design-preview.html` é uma amostra autocontida; não é importado pelo app.
5. Valores novos não devem aparecer soltos em componentes. Primeiro criar o token primitivo, depois o semântico e, por fim, consumi-lo.
6. T-040 e T-042 podem ampliar tokens de componente, mas não devem redefinir os primitivos sem revisão visual e de contraste.
