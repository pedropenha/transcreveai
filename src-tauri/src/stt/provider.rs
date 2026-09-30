//! Contrato `SttProvider` — `specs/architecture/contracts.md` §2.

use super::types::{
    AudioBuffer, HealthReport, ProviderId, SttCapabilities, SttError, SttOptions, Transcript,
};

/// Provedor de speech-to-text (local ou nuvem).
///
/// Em v1 só existe [`super::local::LocalSttProvider`], que embrulha os motores
/// locais (whisper.cpp via transcribe-cpp + modelos ONNX via transcribe-rs).
/// Os provedores de nuvem da v1.1+ (`openai`, `groq`, `openai_compat`,
/// `deepgram`) implementam este mesmo trait sem mudar o orquestrador.
///
/// Nota de implementação: os motores locais são síncronos por natureza —
/// `transcribe` executa inline e o future resolve no primeiro `poll`, o que
/// permite ao orquestrador dirigí-lo de contextos bloqueantes
/// ([`super::orchestrator::SttOrchestrator::transcribe_blocking`]). Provedores
/// que realmente suspendem (HTTP) só funcionam pelo caminho `async`.
#[async_trait::async_trait]
pub trait SttProvider: Send + Sync {
    fn id(&self) -> &ProviderId;
    fn capabilities(&self) -> SttCapabilities;
    async fn transcribe(
        &self,
        audio: AudioBuffer,
        opts: &SttOptions,
    ) -> Result<Transcript, SttError>;
    /// Verifica credencial/modelo com um áudio curto embutido no app.
    /// Contrato para o command `providers_test` da v1.1 — ainda sem chamador.
    #[allow(dead_code)]
    async fn health_check(&self) -> Result<HealthReport, SttError>;
}
