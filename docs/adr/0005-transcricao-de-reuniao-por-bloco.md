# ADR-0005: Transcrição de reunião por bloco inteiro, não por fala

**Date**: 2026-10-07
**Status**: accepted
**Deciders**: Pedro

## Context

A transcrição do ditado saía boa e a do notetaker, com o mesmo modelo, saía
visivelmente pior. Um teste prático em 2026-10-07 levantou a dúvida: era o
modelo (Nemotron Streaming 3.5) ou o pipeline?

Medimos com um harness sobre o áudio real da reunião — `ab_variants`
(`src-tauri/src/meeting/live/ab_variants.rs`) corta os blocos usando o VAD e
o segmentador **de produção**, e `scripts/meeting-ab.ps1` transcreve cada
corte em cada modelo. 5 min da trilha `system`, 72 falas detectadas,
3 modelos × 4 variantes = 450 transcrições.

O resultado separou as duas perguntas:

| variante | o que é                                 |
| -------- | --------------------------------------- |
| `exact`  | o pipeline de então: corte rente ao VAD |
| `padded` | o mesmo + 450 ms de pré-roll/cauda      |
| `whole`  | uma chamada por bloco selado de 60 s    |
| `full`   | uma chamada para a trilha inteira       |

Achados:

- **Fatiar custa fala.** Em `exact`, 11 das 72 falas (15%) não produziram
  texto nenhum no Nemotron. Em `whole`, zero.
- **Fatiar inventa texto.** Whisper e Parakeet alucinaram muletas em inglês
  nas bordas dos cortes curtos ("Just legally.", "Yeah.", "This is
  legally."). Em `whole` e `full`, nenhuma ocorrência.
- **Fatiar quebra frases.** `exact`: "Eu fico madrugada | Maratona dando mão
  de série". `whole`: "Aí eu fico madruga / Fico maratonando um monte de
  série".
- **Fatiar é mais lento.** 72 chamadas custam mais que 5: o `whole`
  processou 300 s de áudio em 6,7 s (Parakeet) contra 21,4 s para os 198 s
  de `exact`.
- **O padding não resolve.** Muda 49–61% dos cortes, mas troca um erro por
  outro; o único ganho objetivo foi reduzir os cortes vazios do Nemotron de
  11 para 6. Não era a correção barata que parecia.
- **`full` não ganha de `whole`.** O bloco de 60 s já dá contexto bastante.
- **O modelo também pesa, mas por conta própria.** O Nemotron erra os mesmos
  trechos em _todas_ as variantes ("o Thiago **Summarathon** sem internet"
  contra "o Thiago e a Silmara estão sem internet" no Whisper e no Parakeet)
  — é um eixo independente deste ADR, resolvido trocando o modelo de
  reunião.

O que o app tinha gravado de fato era pior ainda que o `exact` (527 palavras
contra 569–604), porque os chunks ao vivo entregavam ao motor _sub-porções_
de uma fala, podendo começar no meio da palavra.

## Decision

Um bloco selado chega ao motor **inteiro, numa chamada**. O VAD deixa de
fatiar e passa a responder só duas perguntas: existe fala no bloco, e onde
ela começa e termina (para aparar o silêncio das pontas, com 450 ms de folga
de cada lado). Pausas internas ficam dentro da chamada — são o contexto que
deixa o motor fechar a frase.

Os chunks ao vivo de ~8 s deixam de existir. A única coisa que ainda divide
um bloco é um intervalo de ditado, e ele divide _nas fronteiras do
intervalo_.

Além disso, os segmentos de reunião passam a rodar o mesmo pipeline de texto
determinístico do ditado (`pipeline::run_transcript`: normalizar →
dicionário → limpeza `light`), menos a etapa de comandos de voz — numa
reunião ninguém está comandando o app.

## Alternatives Considered

### Alternativa 1: manter o ao vivo em ≤ 10 s, fundindo falas até 30 s

- **Pros**: preserva o atraso alvo da FR-009-15 e a granularidade fina das
  linhas; não exige emendar spec nem AC.
- **Cons**: captura só parte do ganho — continua cortando a cada 30 s, e a
  medição mostrou que o ganho vem de entregar o bloco inteiro.
- **Why not**: o dono do produto escolheu qualidade sobre latência.

### Alternativa 2: ao vivo provisório + bloco corrige depois

- **Pros**: texto rápido e qualidade final; o melhor dos dois.
- **Cons**: exige substituir linhas no banco e na UI, não só inserir — bem
  mais trabalho e uma fonte nova de inconsistência.
- **Why not**: adiado. É a evolução natural se o atraso de 60 s incomodar.

### Alternativa 3: trocar só o modelo

- **Pros**: custo zero, já resolve os erros de nome.
- **Cons**: não toca nos 15% de falas perdidas nem nas alucinações de borda,
  que são do fatiamento e aparecem nos três modelos.
- **Why not**: resolve um eixo de dois. Foi feito _também_, não em vez disso.

## Consequences

### Positive

- Nenhuma fala perdida por corte, nenhuma alucinação de borda.
- Frases chegam inteiras ao resumo, que é o produto final do notetaker.
- Transcrição mais rápida e com menos chamadas ao motor.
- Ao vivo e pós-processamento usam o mesmo caminho, então um bloco
  transcrito tarde lê igual a um transcrito na hora.
- Menos código: a maquinaria de cobertura/dedup entre chunk e bloco
  (`live/process.rs`, 770 linhas) deixou de ter razão de existir.

### Negative

- **O texto só aparece quando o bloco sela (~60 s), não em ≤ 10 s.** É a
  troca aceita de propósito; FR-009-15 foi corrigida.
- **As linhas do transcript são por bloco**, não por fala: um bloco de 60 s
  vira uma linha de 60 s. Timestamps finos por frase exigiriam
  `Transcript.segments`, que os motores locais não preenchem na v1.
- Uma chamada carrega 60 s de áudio: o timeout por chamada subiu de
  `dictation_timeout` para `block_timeout` (tempo real + 30 s de folga),
  porque a fórmula do ditado dava 21 s para um bloco que o Whisper Turbo
  leva 17 s para transcrever numa GPU quente.

### Risks

- **Uma falha custa 60 s de áudio em vez de 3 s.** Mitigação: o bloco que
  falha mantém `transcribed = 0`, que _é_ a fila do pós-processamento — nada
  se perde, só atrasa.
- **Bloco mudo indo ao motor** quando o VAD não inicializa (o fallback manda
  o bloco inteiro). Mitigação: o filtro de alucinação do motor derruba saída
  de silêncio puro; e sem VAD a alternativa seria descartar fala real.
- **Ditado dentro de um bloco só visto pelo pós-processamento não é
  excluído** — o pós-passe não tem o rastreador de intervalos, que morre com
  a sessão. Essa lacuna é anterior a este ADR e segue aberta; o caminho ao
  vivo continua honrando o AC-009-03.
