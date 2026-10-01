# Contratos: provedores, APIs externas e IPC

> Os trechos de código são **ilustrativos** — definem o que cada contrato precisa expressar. A forma idiomática segue as rules: IDs como _newtypes_ (`ProviderId`, `SessionId`, `MeetingId`, `DetectionId`) e estados como enums com `match` exaustivo (`rules/rust/patterns.md`); erros tipados com `thiserror` nos módulos e `anyhow` na camada de aplicação, sem `unwrap()` em produção (`rules/rust/coding-style.md`); dados externos convertidos em tipos na fronteira ("parse, don't validate", `rules/rust/security.md`).

## 1. Tipos de áudio

```rust
/// Sempre PCM f32, mono, 16 kHz após o engine de áudio.
pub struct AudioBuffer {
    pub samples: Arc<[f32]>,
    pub sample_rate: u32,      // 16_000
    pub started_at: Instant,
}
```

## 2. `SttProvider`

```rust
#[async_trait]
pub trait SttProvider: Send + Sync {
    fn id(&self) -> &ProviderId;
    fn capabilities(&self) -> SttCapabilities;
    async fn transcribe(&self, audio: AudioBuffer, opts: &SttOptions) -> Result<Transcript, SttError>;
    /// Verifica credencial/modelo com um áudio curto embutido no app.
    async fn health_check(&self) -> Result<HealthReport, SttError>;
}

pub struct SttCapabilities {
    pub local: bool,
    pub max_upload_bytes: Option<u64>,   // OpenAI: 25 MB
    pub max_audio_secs: Option<u32>,
    pub supports_prompt: bool,           // dicas de vocabulário
    pub supports_diarization: bool,
    pub supports_segments: bool,         // timestamps por segmento
    pub languages: LanguageSupport,
}

pub struct SttOptions {
    pub language: Option<String>,        // "pt", "en" ou None = auto
    pub vocabulary_hints: Vec<String>,   // do dicionário
    pub diarize: bool,
    pub timeout: Duration,
}

pub struct Transcript {
    pub text: String,
    pub language: Option<String>,
    pub segments: Vec<Segment>,          // start_ms, end_ms, text, speaker: Option<String>, no_speech_prob: Option<f32>
    pub provider_latency_ms: u32,
}

#[derive(Debug, thiserror::Error)]
pub enum SttError {
    #[error("network error")] Network(#[source] reqwest::Error),
    #[error("invalid or unauthorized API key")] Auth,
    #[error("rate limited")] RateLimited { retry_after: Option<Duration> },
    #[error("timed out")] Timeout,
    #[error("audio too large for provider")] AudioTooLarge,
    #[error("local model not ready")] ModelNotReady,
    #[error("blocked by offline mode")] Offline,
    #[error("provider error: {0}")] Provider(String),
}
```

Erros recuperáveis (`Network`, `Timeout`, `RateLimited`, `Offline`) acionam o **fallback provider** se configurado.

## 3. `LlmProvider`

```rust
#[async_trait]
pub trait LlmProvider: Send + Sync {
    fn id(&self) -> &ProviderId;
    async fn complete(&self, req: LlmRequest) -> Result<LlmResponse, LlmError>;
    async fn health_check(&self) -> Result<HealthReport, LlmError>;
}

pub struct LlmRequest {
    pub system: String,
    pub messages: Vec<LlmMessage>,     // role + content
    pub max_tokens: u32,
    pub temperature: f32,              // 0.0–0.3 para limpeza
    pub timeout: Duration,
    pub purpose: LlmPurpose,           // Cleanup | Command | Summary | Title — para métricas
}
```

Sem ferramentas/tool use: o LLM só devolve texto. Nada que ele retorne é executado.

## 4. Mapeamento das APIs externas

> Modelos e preços mudam; o app mantém um **catálogo editável** (`resources/catalog.json`) com modelos sugeridos e custo estimado por minuto, e o usuário sempre pode digitar outro nome de modelo.

| Provedor              | Endpoint                                                                                                                    | Autenticação                                  | Observações                                                                                                                                                                             |
| --------------------- | --------------------------------------------------------------------------------------------------------------------------- | --------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| **OpenAI STT**        | `POST https://api.openai.com/v1/audio/transcriptions` (multipart: `file`, `model`, `language`, `prompt`, `response_format`) | `Authorization: Bearer`                       | Modelos: `gpt-4o-mini-transcribe` (mais barato), `gpt-4o-transcribe`, `gpt-4o-transcribe-diarize` (reuniões), `whisper-1` (único com timestamps por palavra). Limite 25 MB por arquivo. |
| **Groq STT**          | `POST https://api.groq.com/openai/v1/audio/transcriptions`                                                                  | Bearer                                        | Mesmo formato da OpenAI. `whisper-large-v3-turbo` (muito rápido).                                                                                                                       |
| **Deepgram**          | `POST https://api.deepgram.com/v1/listen?model=…&language=…&smart_format=true&diarize=…` (corpo binário)                    | `Authorization: Token`                        | Streaming via WebSocket (P2). Dicas via `keyterm`.                                                                                                                                      |
| **Compatível OpenAI** | `POST {base_url}/audio/transcriptions`                                                                                      | Bearer opcional                               | Cobre servidores locais (speaches/faster-whisper-server, LocalAI) e outros.                                                                                                             |
| **OpenAI-compat LLM** | `POST {base_url}/chat/completions`                                                                                          | Bearer                                        | OpenAI, Groq, OpenRouter, Ollama (`http://localhost:11434/v1`).                                                                                                                         |
| **Anthropic LLM**     | `POST https://api.anthropic.com/v1/messages`                                                                                | `x-api-key` + `anthropic-version: 2023-06-01` | Modelo configurável (ex.: um Haiku para limpeza rápida, um Sonnet para resumos).                                                                                                        |

**Áudio para upload**: WAV 16 kHz mono 16-bit (~1,9 MB/min). Áudio > 20 MB é dividido em pontos de silêncio (VAD) em blocos ≤ 10 min, enviados com concorrência ≤ 3 e concatenados na ordem. (P1: comprimir para FLAC/Opus antes do envio.)

## 5. IPC Tauri

**Formato de resposta**: todo command responde no envelope consistente de `rules/common/patterns.md` / `rules/rust/patterns.md` — `{ "status": "ok", "data": … }` ou `{ "status": "error", "code": "…", "message": "…" }`. A `message` é amigável ao usuário e não vaza caminhos, stack traces nem segredos; o detalhe vai para o log (`rules/rust/security.md`). Toda entrada de command é validada no núcleo (fronteira do sistema); formulários e JSON importados no frontend são validados com Zod (`rules/typescript/coding-style.md`).

### Commands (frontend → núcleo)

| Command                                                                                                                                                           | Entrada                                                                           | Saída                                                                                       |
| ----------------------------------------------------------------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------- |
| `dictation_start` / `dictation_stop` / `dictation_cancel`                                                                                                         | `{ mode }`                                                                        | `()`                                                                                        |
| `settings_get` / `settings_update`                                                                                                                                | patch JSON                                                                        | `Settings`                                                                                  |
| `providers_list` / `providers_save` / `providers_delete` / `providers_test`                                                                                       | …                                                                                 | `Provider[]` / `HealthReport`                                                               |
| `secret_set`                                                                                                                                                      | `{ provider_id, secret }` — `secret` vazio remove a chave                         | `Option<String>` — aviso de formato não-bloqueante (FR-011-05); **não existe `secret_get`** |
| `secret_clear`                                                                                                                                                    | `{ provider_id }`                                                                 | `()`                                                                                        |
| `secret_hint`                                                                                                                                                     | `{ provider_id }`                                                                 | `Option<String>` — máscara `••••` + sufixo de até `len/4` chars (máx. 4); nunca a chave     |
| `models_catalog` / `models_download` / `models_cancel` / `models_delete` / `models_import`                                                                        | …                                                                                 | …                                                                                           |
| `history_list` / `history_search` / `history_delete` / `history_reinsert` / `history_retry`                                                                       | …                                                                                 | …                                                                                           |
| `dictionary_*`, `snippets_*`, `profiles_*`, `transforms_*`                                                                                                        | CRUD                                                                              | …                                                                                           |
| `notes_*`                                                                                                                                                         | CRUD + `notes_export`                                                             | …                                                                                           |
| `meeting_start` / `meeting_stop` / `meeting_pause` / `meeting_resume` / `meeting_get` / `meeting_list` / `meeting_regenerate_summary` / `meeting_export_markdown` | …                                                                                 | …                                                                                           |
| `detector_respond`                                                                                                                                                | `{ detection_id, action: start \| start_mic_only \| dismiss \| always \| never }` | `()`                                                                                        |
| `hotkey_capture_begin` / `hotkey_capture_end`                                                                                                                     | —                                                                                 | combinação capturada                                                                        |
| `flowbar_set_hover`                                                                                                                                               | `{ hovering }`                                                                    | `()` (controle de click-through)                                                            |

### Events (núcleo → frontend)

| Evento               | Payload                                                            |
| -------------------- | ------------------------------------------------------------------ |
| `session://state`    | `{ session_id, state, mode, error? }`                              |
| `audio://level`      | `{ rms: f32[] }` a 30 Hz, só durante gravação                      |
| `session://result`   | `{ session_id, final_text, inserted, insertion_status?, insertion_method?, insertion_fallback?, insert_ms? }` |
| `detector://meeting` | `{ detection_id, app_label, exe, icon, started_at }` / `{ ended }` |
| `meeting://state`    | `{ meeting_id, status, elapsed_ms }`                               |
| `meeting://segment`  | `Segment` (transcrição ao vivo)                                    |
| `meeting://progress` | `{ meeting_id, step, pct }`                                        |
| `models://progress`  | `{ model_id, bytes, total }`                                       |
| `toast://show`       | `{ kind, message, action? }`                                       |
| `insertion://clipboard-only-warning` | `{ reason: requested \| elevated_target \| no_foreground_target \| window_changed, exe_name? }` — texto ficou no clipboard, nada foi injetado (FR-005-07/08/09) |
| `settings://changed` | patch                                                              |

Tipos TypeScript são gerados a partir dos structs Rust (`specta`/`tauri-specta`) para evitar divergência.
