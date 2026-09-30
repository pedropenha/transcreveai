# F003 — Motores de transcrição (local e nuvem)

**Status**: Draft · **Release**: MVP (whisper local + OpenAI/Groq/compatível) · v1.0 (Parakeet, Deepgram, diarização)
**Depende de**: F011 (segredos)

## Contexto

O usuário escolhe entre transcrever **localmente** (grátis, offline, privado; depende do hardware) ou usar uma **API paga com a própria chave** (mais rápida/precisa em máquinas fracas). Pode usar provedores diferentes para ditado e reuniões, e definir um fallback.

## Histórias

- **US-003-01** Quero transcrever 100 % offline, sem enviar áudio a ninguém.
- **US-003-02** Quero colar minha chave da OpenAI (ou Groq) e usar um modelo pago.
- **US-003-03** Quero que o app recomende o modelo local adequado ao meu computador e o baixe para mim.
- **US-003-04** Quero um fallback: se a nuvem falhar, usar o modelo local.
- **US-003-05** Quero saber quanto estou gastando (estimativa) com a API.

## Provedores

| Tipo                                                      | Release   | Modelos sugeridos                                                                                             | Notas                                                                                           |
| --------------------------------------------------------- | --------- | ------------------------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------- |
| `local_whisper` (whisper.cpp)                             | MVP       | `tiny`, `base`, `small`, `medium`, `large-v3-turbo` (quantizações q5_0/q8_0)                                  | GPU via Vulkan (build alternativo), CPU com AVX2.                                               |
| `local_parakeet` (NVIDIA Parakeet TDT 0.6B v3, ONNX int8) | v1.0 (P1) | `parakeet-tdt-0.6b-v3`                                                                                        | Muito rápido em CPU; 25 idiomas europeus, inclui português.                                     |
| `openai`                                                  | MVP       | `gpt-4o-mini-transcribe` (padrão, mais barato), `gpt-4o-transcribe`, `gpt-4o-transcribe-diarize`, `whisper-1` | 25 MB por arquivo.                                                                              |
| `groq`                                                    | MVP       | `whisper-large-v3-turbo`, `whisper-large-v3`                                                                  | API compatível com OpenAI; baixa latência.                                                      |
| `openai_compat`                                           | MVP       | livre                                                                                                         | `base_url` + modelo + chave opcional (servidores locais tipo speaches/faster-whisper, LocalAI). |
| `deepgram`                                                | P1        | `nova-3`                                                                                                      | Streaming e diarização nativos.                                                                 |

Catálogo (`resources/catalog.json`) guarda para cada modelo: id, motor, URL de download, **SHA-256**, tamanho, idiomas, RAM/VRAM aproximadas, custo estimado/min (nuvem). O catálogo é atualizável sem nova versão do app (P2), mas o SHA-256 é sempre verificado.

## Requisitos funcionais

### Configuração

- **FR-003-01** Tela "Modelos & Provedores" lista provedores configurados com: tipo, modelo, local/nuvem, status (pronto, baixando, erro, sem chave), uso (ditado/reunião/fallback).
- **FR-003-02** Adicionar provedor em nuvem: escolher tipo → colar chave (campo mascarado) → escolher/digitar modelo → **Testar conexão** (envia áudio embutido de ~2 s "teste de transcrição" e mostra texto retornado + latência) → salvar. Chave vai direto para o cofre (F011).
- **FR-003-03** Selecionar, separadamente: provedor de **ditado**, provedor de **reuniões** e provedor de **fallback** (opcional).
- **FR-003-04** Com o **modo offline** ligado (F011), provedores em nuvem ficam desabilitados e a seleção cai no local; se não houver local, a UI avisa.

### Modelos locais

- **FR-003-05** Detectar hardware na primeira execução: RAM, núcleos, AVX2, GPU compatível com Vulkan/CUDA e VRAM. Recomendação:
  | Hardware | Recomendado |
  |---|---|
  | GPU dedicada ≥ 4 GB VRAM | whisper `large-v3-turbo` (q5_0) na GPU |
  | CPU moderna, ≥ 16 GB RAM | Parakeet v3 (v1.0) ou whisper `small` |
  | ≥ 8 GB RAM | whisper `base` / `small` q5 |
  | abaixo disso | sugerir nuvem |
- **FR-003-06** Download com barra de progresso (`models://progress`), **retomável** (HTTP Range), cancelável, checagem de espaço em disco antes, verificação **SHA-256** ao final (falha → apaga e mostra erro).
- **FR-003-07** Importar modelo de arquivo local (para redes restritas), com verificação de formato; SHA-256 exibido.
- **FR-003-08** Excluir modelo libera o disco; se era o provedor em uso, pedir outro.
- **FR-003-09** Ciclo de vida: carregar no primeiro uso (ou ao iniciar, se "Pré-carregar" ligado); manter na memória; descarregar após N min ocioso (padrão 15, configurável, "nunca").
- **FR-003-10** Aceleração: usar GPU se disponível e compatível; se a inicialização falhar, cair para CPU automaticamente e registrar no log; exibir "GPU (Vulkan)" / "CPU" na UI.

### Transcrição

- **FR-003-11** Antes de qualquer provedor: aparar silêncio no início/fim com VAD; se não houver fala, não chamar o provedor (FR-002-14).
- **FR-003-12** Dicas de vocabulário: termos do dicionário (F004) enviados como `initial_prompt` (whisper, limitado a ~200 tokens, priorizando os mais usados), `prompt` (OpenAI/Groq) ou `keyterm` (Deepgram).
- **FR-003-13** **Filtro de alucinação**: descartar segmentos com `no_speech_prob` alto e/ou energia baixa que correspondam à lista de bloqueio (pt/en), ex.: "Legendas pela comunidade Amara.org", "Obrigado por assistir", "Inscreva-se no canal", "Thanks for watching", "[Música]". Lista editável.
- **FR-003-14** Áudio longo: local → processado em janelas nativas do motor; nuvem → dividir em pontos de silêncio em blocos ≤ 10 min / ≤ 20 MB, enviar com concorrência ≤ 3 e concatenar em ordem.
- **FR-003-15** Timeouts padrão: ditado 15 s + 1 s por 10 s de áudio; reunião 120 s por bloco. Configuráveis.
- **FR-003-16** Erros: `401/403` → "Chave inválida ou sem permissão" (sem retry); `429` → retry com backoff exponencial (máx. 2) respeitando `Retry-After`, depois fallback; `5xx`/rede/timeout → 1 retry, depois fallback; sem fallback → erro da sessão (áudio preservado).
- **FR-003-17** Registrar em cada sessão: provedor, modelo, idioma detectado, latência do provedor.
- **FR-003-18** Uso e custo estimado: minutos transcritos por provedor por dia/mês; custo = minutos × preço do catálogo (rotulado como **estimativa**).
- **FR-003-19** P1: diarização para reuniões (provedor com `supports_diarization` ou diarização local sherpa-onnx na trilha `system`).
- **FR-003-20** P2: streaming (OpenAI Realtime / Deepgram WebSocket) para transcrição ao vivo em reuniões e texto aparecendo enquanto se fala.

## Requisitos não funcionais

- **NFR-003-01** RTF (tempo de processamento ÷ duração do áudio) com large-v3-turbo q5 em GPU ≤ 0,1; com `small` em CPU 8 núcleos ≤ 0,3.
- **NFR-003-02** WER e latência por modelo medidos num conjunto de fixtures pt-BR/en (sotaques variados, termos técnicos), com baseline e detecção de regressão pela skill `benchmark`.
- **NFR-003-03** Chaves nunca aparecem em logs, erros exibidos ou relatórios.

## Critérios de aceitação

- **AC-003-01** _Dado_ nenhum provedor configurado, _quando_ escolho "Local" no onboarding numa máquina sem GPU com 16 GB, _então_ o app recomenda um modelo compatível, baixa com progresso, verifica o hash e transcreve "teste" corretamente.
- **AC-003-02** _Dado_ o cabo de rede desconectado e provedor local, _quando_ dito, _então_ a transcrição funciona e **nenhuma** conexão de rede é aberta (verificado com monitor de rede no teste).
- **AC-003-03** _Quando_ colo uma chave inválida da OpenAI e clico em Testar, _então_ vejo "Chave inválida" em ≤ 5 s e a chave não é salva até eu confirmar.
- **AC-003-04** _Dado_ OpenAI como principal e whisper local como fallback, _quando_ a OpenAI retorna 503 duas vezes, _então_ o texto é transcrito localmente e o histórico registra o provedor de fallback.
- **AC-003-05** _Dado_ 3 s de silêncio gravado, _então_ nada é inserido e nenhum texto como "Legendas pela comunidade Amara.org" aparece.
- **AC-003-06** _Dado_ "Kubernetes" e "Transcreve.ai" no dicionário, _quando_ dito "suba o transcreve ai no kubernetes", _então_ os termos saem com a grafia do dicionário em ≥ 90 % das fixtures.
- **AC-003-07** _Dado_ uma gravação de 25 min enviada à OpenAI, _então_ ela é dividida em blocos < 25 MB e o texto final está completo e em ordem.
- **AC-003-08** _Dado_ o download interrompido em 60 %, _quando_ reabro o app e retomo, _então_ o download continua de onde parou.

## Casos de borda

- Driver de GPU quebrado → crash nativo no carregamento: carregar o modelo num processo/thread com proteção e marcar "GPU desabilitada" após falha.
- Modelo corrompido no disco → hash conferido ao carregar (rápido: tamanho + hash em segundo plano periodicamente).
- Proxy corporativo → respeitar `HTTPS_PROXY`/proxy do sistema.
- Base URL com `http://` → só permitido para `localhost`/`127.0.0.1`/`::1` (F011).

## Perguntas em aberto

- `[NEEDS CLARIFICATION]` Qual provedor pré-selecionar se o usuário pular o onboarding? **Proposta**: nenhum; a Flow Bar mostra "Configure a transcrição" ao primeiro uso.
- `[NEEDS CLARIFICATION]` Distribuir o build CUDA além do Vulkan? **Proposta**: só CPU + Vulkan no v1 (cobre NVIDIA/AMD/Intel); CUDA como P2.
