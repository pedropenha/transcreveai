//! Tipos do contrato de provedores STT — `specs/architecture/contracts.md`
//! §1 (tipos de áudio) e §2 (`SttProvider`).

use serde::{Deserialize, Serialize};
use std::fmt;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Identificador estável de um provedor (`local_whisper`, `openai`, `groq`,
/// `openai_compat`, ...). Newtype conforme `rules/rust/patterns.md` — nunca um
/// `String` solto. Compartilhado com o futuro `LlmProvider` (contracts §3).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ProviderId(String);

impl ProviderId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }
}

impl fmt::Display for ProviderId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<&str> for ProviderId {
    fn from(id: &str) -> Self {
        Self(id.to_string())
    }
}

/// Sempre PCM f32, mono, 16 kHz após o engine de áudio (contracts §1).
#[derive(Clone, Debug)]
pub struct AudioBuffer {
    pub samples: Arc<[f32]>,
    pub sample_rate: u32,
    pub started_at: Instant,
}

impl AudioBuffer {
    /// Taxa fixa do pipeline de áudio do app.
    pub const SAMPLE_RATE: u32 = 16_000;

    /// Buffer de uma sessão de ditado: PCM f32 mono a 16 kHz capturado agora.
    pub fn dictation(samples: Vec<f32>) -> Self {
        Self {
            samples: Arc::from(samples),
            sample_rate: Self::SAMPLE_RATE,
            started_at: Instant::now(),
        }
    }

    pub fn duration_secs(&self) -> f64 {
        self.samples.len() as f64 / self.sample_rate as f64
    }
}

/// Quais idiomas o provedor consegue produzir.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LanguageSupport {
    /// O provedor detecta o idioma do áudio por conta própria
    /// (`SttOptions::language == None`).
    pub auto_detect: bool,
    /// Conjunto fechado de códigos ISO 639/BCP-47; vazio = sem restrição
    /// conhecida (o modelo decide).
    pub languages: Vec<String>,
}

/// Capacidades declaradas pelo provedor (contracts §2); guiam a UI e a
/// orquestração (ex.: `supports_prompt` libera `vocabulary_hints`).
/// Campos de limite/upload e diarizacao sao contrato para provedores de
/// nuvem (v1.1+); na v1 o orquestrador le apenas local/prompt/segments.
#[allow(dead_code)]
#[derive(Clone, Debug, Default)]
pub struct SttCapabilities {
    pub local: bool,
    pub max_upload_bytes: Option<u64>,
    pub max_audio_secs: Option<u32>,
    pub supports_prompt: bool,
    pub supports_diarization: bool,
    pub supports_segments: bool,
    pub languages: LanguageSupport,
}

/// Opções por chamada de transcrição (contracts §2). O orquestrador preenche a
/// partir das settings da sessão; o provedor pode ignorar o que não suporta.
#[derive(Clone, Debug)]
pub struct SttOptions {
    /// `"pt"`, `"en"`, ... ou `None` = auto.
    pub language: Option<String>,
    /// Termos do dicionário (`initial_prompt` no whisper; `prompt`/`keyterm`
    /// nos provedores de nuvem).
    pub vocabulary_hints: Vec<String>,
    pub diarize: bool,
    pub timeout: Duration,
}

/// Um segmento de transcrição com timestamps (vazio na v1 — os motores locais
/// devolvem só o texto final; preenchido pelos provedores de nuvem/diarização
/// da v1.1+).
#[allow(dead_code)]
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Segment {
    pub start_ms: u64,
    pub end_ms: u64,
    pub text: String,
    pub speaker: Option<String>,
    pub no_speech_prob: Option<f32>,
}

/// Resultado de uma transcrição de provedor (contracts §2).
#[derive(Clone, Debug, Default)]
pub struct Transcript {
    pub text: String,
    /// Idioma da saída quando há evidência (hint aplicado, LID do modelo ou
    /// detecção por texto); `None` quando desconhecido.
    pub language: Option<String>,
    /// Vazio na v1 — os motores locais não expõem segmentos ainda.
    #[allow(dead_code)]
    pub segments: Vec<Segment>,
    /// Tempo de inferência/chamada do provedor vencedor (FR-003-17).
    pub provider_latency_ms: u32,
}

impl Transcript {
    /// Transcrição vazia (ex.: áudio sem amostras não chega ao motor).
    pub fn empty() -> Self {
        Self::default()
    }
}

/// Saúde de um provedor, devolvida por `providers_test` (contracts §2/§5) —
/// o command IPC chega na v1.1 junto com a tela de provedores.
#[allow(dead_code)]
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HealthReport {
    pub ok: bool,
    pub latency_ms: Option<u64>,
    pub detail: Option<String>,
}

/// Erros tipados do provedor (contracts §2). A decisão de retry/fallback vive
/// em `stt::orchestrator` (FR-003-16); aqui ficam só a classificação
/// semântica e a conversão para a camada de aplicação.
///
/// `dead_code`: as variantes de nuvem (`Network`, `Auth`, `RateLimited`,
/// `Timeout`, `AudioTooLarge`, `Offline`) só são produzidas por provedores
/// v1.1+; em v1 nascem `ModelNotReady` e `Provider`.
#[allow(dead_code)]
#[derive(Debug, thiserror::Error)]
pub enum SttError {
    #[error("network error")]
    Network(#[source] reqwest::Error),
    #[error("invalid or unauthorized API key")]
    Auth,
    #[error("rate limited")]
    RateLimited { retry_after: Option<Duration> },
    #[error("timed out")]
    Timeout,
    #[error("audio too large for provider")]
    AudioTooLarge,
    #[error("local model not ready")]
    ModelNotReady,
    #[error("blocked by offline mode")]
    Offline,
    #[error("provider error: {0}")]
    Provider(String),
}

impl SttError {
    /// Erros recuperáveis do contrato §2: acionam o provedor de fallback
    /// quando configurado.
    pub fn is_recoverable(&self) -> bool {
        matches!(
            self,
            Self::Network(_) | Self::Timeout | Self::RateLimited { .. } | Self::Offline
        )
    }

    /// Se um outro provedor pode contornar a falha. Além dos recuperáveis,
    /// cobre `5xx`/falha de inferência mapeados em `Provider`, modelo local
    /// não pronto (`ModelNotReady`) e áudio grande demais para o limite da
    /// nuvem (`AudioTooLarge` pode caber no motor local). Só `Auth` não faz
    /// fallback: credencial inválida é erro de configuração, não de provedor
    /// (FR-003-16: "sem retry").
    pub fn may_fallback(&self) -> bool {
        self.is_recoverable()
            || matches!(
                self,
                Self::Provider(_) | Self::ModelNotReady | Self::AudioTooLarge
            )
    }

    /// Converte para `anyhow` preservando as mensagens históricas exibidas ao
    /// usuário pelo pipeline de ditado (toast `transcription-error`): o
    /// `Provider` carrega a mensagem original do motor intacta.
    pub fn into_anyhow(self) -> anyhow::Error {
        match self {
            Self::Provider(msg) => anyhow::anyhow!(msg),
            Self::ModelNotReady => anyhow::anyhow!("Model is not loaded for transcription."),
            other => anyhow::Error::new(other),
        }
    }
}
