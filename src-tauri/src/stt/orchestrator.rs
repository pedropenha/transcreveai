//! Orquestrador de provedores STT: seleção de provedor, retry e fallback —
//! FR-003-16 / contracts §2. Separado da lógica do motor: o orquestrador só
//! conhece o trait [`SttProvider`].
//!
//! Política (v1 — só STT local; a nuvem entra na v1.1 sem mudar a estrutura):
//! * falha do motor local ou transitória (`Network`/`Timeout`/`Provider`/
//!   `ModelNotReady`) → 1 retentativa no mesmo provedor;
//! * `RateLimited` (429) → até 2 retentativas com backoff exponencial,
//!   respeitando `Retry-After` quando presente;
//! * erro esgotado que admite fallback → provedor de fallback, se configurado;
//! * sem fallback → erro da sessão. A preservação do áudio (WAV + entrada de
//!   histórico para retry) fica na camada de sessão (`actions.rs`), que já
//!   grava o áudio em paralelo antes de conhecer o resultado.

use super::provider::SttProvider;
use super::types::{AudioBuffer, ProviderId, SttError, SttOptions, Transcript};
use futures_util::FutureExt;
use log::{debug, info, warn};
use std::sync::Arc;
use std::time::Duration;

/// Política de retentativas por provedor (FR-003-16).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RetryPolicy {
    /// Retentativas extras no mesmo provedor para erros transitórios
    /// (rede/timeout/5xx) e falhas do motor local — v1: 1.
    pub max_retries: u32,
    /// Retentativas extras para `RateLimited` (429) — v1.1+: máx. 2.
    pub max_rate_limit_retries: u32,
    /// Backoff exponencial para 429 sem `Retry-After`:
    /// `backoff_base * 2^(tentativa-1)`, limitado a `backoff_cap`.
    pub backoff_base: Duration,
    pub backoff_cap: Duration,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_retries: 1,
            max_rate_limit_retries: 2,
            backoff_base: Duration::from_millis(500),
            backoff_cap: Duration::from_secs(30),
        }
    }
}

impl RetryPolicy {
    /// Backoff exponencial para `RateLimited` sem `Retry-After` informado.
    fn backoff(&self, attempts: u32) -> Duration {
        let shift = attempts.saturating_sub(1).min(10);
        self.backoff_base
            .saturating_mul(1 << shift)
            .min(self.backoff_cap)
    }
}

/// O que o orquestrador faz depois de uma falha de provedor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FailureAction {
    /// Repete no mesmo provedor, esperando `delay` antes.
    Retry { delay: Duration },
    /// Troca para o provedor de fallback configurado.
    SwitchToFallback,
    /// Falha definitiva: vira erro da sessão (o chamador preserva o áudio).
    Fail,
}

/// Decide o destino de uma falha. `attempts` é quantas vezes o provedor
/// **atual** já foi chamado nesta sessão; `fallback_available` indica que há
/// um provedor de fallback ainda não utilizado.
fn failure_action(
    error: &SttError,
    attempts: u32,
    policy: &RetryPolicy,
    fallback_available: bool,
) -> FailureAction {
    fn fallback_or_fail(error: &SttError, fallback_available: bool) -> FailureAction {
        if fallback_available && error.may_fallback() {
            FailureAction::SwitchToFallback
        } else {
            FailureAction::Fail
        }
    }

    match error {
        // Credencial inválida é determinística e não é contornada por outro
        // provedor: erro direto (FR-003-16: 401/403 → sem retry).
        SttError::Auth => FailureAction::Fail,
        // Nada a re-tentar no mesmo provedor; o fallback decide.
        SttError::Offline | SttError::AudioTooLarge => fallback_or_fail(error, fallback_available),
        // 429: backoff exponencial respeitando Retry-After, depois fallback.
        SttError::RateLimited { retry_after } if attempts <= policy.max_rate_limit_retries => {
            FailureAction::Retry {
                delay: retry_after.unwrap_or_else(|| policy.backoff(attempts)),
            }
        }
        // Transitórios e falhas do motor local: 1 retentativa, depois fallback
        // (ou erro da sessão se não houver fallback).
        _ if attempts <= policy.max_retries => FailureAction::Retry {
            delay: Duration::ZERO,
        },
        _ => fallback_or_fail(error, fallback_available),
    }
}

/// Resultado vencedor da orquestração, com a telemetria que a sessão registra
/// (FR-003-17: provedor, tentativas, uso de fallback).
#[derive(Clone, Debug)]
pub struct TranscriptionOutcome {
    pub transcript: Transcript,
    pub provider_id: ProviderId,
    /// Quantas chamadas o provedor vencedor recebeu nesta sessão.
    pub provider_attempts: u32,
    /// Se o resultado veio do provedor de fallback.
    pub used_fallback: bool,
}

/// Seleciona o provedor e aplica retry único/fallback sobre o trait
/// [`SttProvider`]. Independe de Tauri — testável com providers mockados.
pub struct SttOrchestrator {
    primary: Arc<dyn SttProvider>,
    fallback: Option<Arc<dyn SttProvider>>,
    /// Politica de retry/fallback; publica para tuning por modo de uso
    /// (ditado vs. reuniao) e para os testes.
    pub policy: RetryPolicy,
}

impl SttOrchestrator {
    /// Orquestrador da v1: provedor primário + fallback opcional (nenhum hoje
    /// — entra com os provedores de nuvem na v1.1).
    pub fn new(primary: Arc<dyn SttProvider>, fallback: Option<Arc<dyn SttProvider>>) -> Self {
        Self {
            primary,
            fallback,
            policy: RetryPolicy::default(),
        }
    }

    /// Sobrescreve a política de retry (testes e tuning por modo de uso).
    #[allow(dead_code)] // gancho de tuning publico para modos de uso da v1.1+
    pub fn with_policy(mut self, policy: RetryPolicy) -> Self {
        self.policy = policy;
        self
    }

    /// Timeout de ditado: 15 s + 1 s por 10 s de áudio (FR-003-15). O motor
    /// local da v1 não impõe deadline; o valor viaja em `SttOptions` para os
    /// provedores de nuvem da v1.1+ e para a telemetria.
    pub fn dictation_timeout(audio_secs: f64) -> Duration {
        Duration::from_secs(15) + Duration::from_secs_f64(audio_secs / 10.0)
    }

    /// Transcreve `audio` aplicando a política de retry/fallback.
    pub async fn transcribe(
        &self,
        audio: AudioBuffer,
        opts: &SttOptions,
    ) -> Result<TranscriptionOutcome, SttError> {
        let mut provider = &self.primary;
        let mut used_fallback = false;
        let mut provider_attempts = 0u32;

        let caps = provider.capabilities();
        debug!(
            "STT session via provider '{}': local={}, prompt={}, segments={}, timeout={:?}",
            provider.id(),
            caps.local,
            caps.supports_prompt,
            caps.supports_segments,
            opts.timeout
        );

        loop {
            provider_attempts += 1;
            match provider.transcribe(audio.clone(), opts).await {
                Ok(transcript) => {
                    info!(
                        "STT provider '{}' produced {} chars in {}ms \
                         (attempt {}, fallback={}, language={:?})",
                        provider.id(),
                        transcript.text.len(),
                        transcript.provider_latency_ms,
                        provider_attempts,
                        used_fallback,
                        transcript.language
                    );
                    return Ok(TranscriptionOutcome {
                        transcript,
                        provider_id: provider.id().clone(),
                        provider_attempts,
                        used_fallback,
                    });
                }
                Err(error) => {
                    match failure_action(
                        &error,
                        provider_attempts,
                        &self.policy,
                        self.fallback.is_some() && !used_fallback,
                    ) {
                        FailureAction::Retry { delay } => {
                            warn!(
                                "STT provider '{}' failed (attempt {}): {}; retrying in {:?}",
                                provider.id(),
                                provider_attempts,
                                error,
                                delay
                            );
                            if !delay.is_zero() {
                                tokio::time::sleep(delay).await;
                            }
                        }
                        FailureAction::SwitchToFallback => match &self.fallback {
                            Some(fallback) => {
                                warn!(
                                    "STT provider '{}' failed (attempt {}): {}; \
                                     switching to fallback '{}'",
                                    provider.id(),
                                    provider_attempts,
                                    error,
                                    fallback.id()
                                );
                                provider = fallback;
                                used_fallback = true;
                                provider_attempts = 0;
                            }
                            // Defensivo: a decisão só escolhe fallback quando
                            // existe um configurado e ainda não usado.
                            None => return Err(error),
                        },
                        FailureAction::Fail => {
                            warn!(
                                "STT provider '{}' failed definitively (attempt {}): {}",
                                provider.id(),
                                provider_attempts,
                                error
                            );
                            return Err(error);
                        }
                    }
                }
            }
        }
    }

    /// Dirige [`transcribe`](Self::transcribe) a partir de um contexto
    /// bloqueante (pipeline de ditado, CLI, `spawn_blocking`).
    ///
    /// Os provedores locais da v1 executam inline — o future do orquestrador
    /// resolve no primeiro `poll`. Se um provedor futuro realmente suspender
    /// neste caminho (provedor de nuvem, backoff de rate limit), retorna erro
    /// em vez de bloquear uma thread sem reactor: quem for async deve usar o
    /// caminho `transcribe`.
    pub fn transcribe_blocking(
        &self,
        audio: AudioBuffer,
        opts: &SttOptions,
    ) -> Result<TranscriptionOutcome, SttError> {
        self.transcribe(audio, opts)
            .now_or_never()
            .unwrap_or_else(|| {
                Err(SttError::Provider(
                    "STT provider suspended on the synchronous transcription path".to_string(),
                ))
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stt::types::{HealthReport, SttCapabilities};
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;

    /// Provedor de roteiro: consome uma fila de respostas prontas e conta as
    /// chamadas. O future resolve inline como o motor local (resolve no
    /// primeiro `poll`), então também cobre `transcribe_blocking`.
    struct MockProvider {
        id: ProviderId,
        calls: AtomicUsize,
        script: Mutex<VecDeque<Result<Transcript, SttError>>>,
    }

    impl MockProvider {
        fn new(id: &str, script: Vec<Result<Transcript, SttError>>) -> Arc<Self> {
            Arc::new(Self {
                id: ProviderId::from(id),
                calls: AtomicUsize::new(0),
                script: Mutex::new(script.into()),
            })
        }

        fn calls(&self) -> usize {
            self.calls.load(Ordering::SeqCst)
        }
    }

    #[async_trait::async_trait]
    impl SttProvider for MockProvider {
        fn id(&self) -> &ProviderId {
            &self.id
        }

        fn capabilities(&self) -> SttCapabilities {
            SttCapabilities::default()
        }

        async fn transcribe(
            &self,
            _audio: AudioBuffer,
            _opts: &SttOptions,
        ) -> Result<Transcript, SttError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.script
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or_else(|| Err(SttError::Provider("script exhausted".to_string())))
        }

        async fn health_check(&self) -> Result<HealthReport, SttError> {
            Ok(HealthReport {
                ok: true,
                latency_ms: Some(0),
                detail: None,
            })
        }
    }

    /// Provedor que suspende de verdade (simula um provedor de nuvem).
    struct SuspendingProvider {
        id: ProviderId,
    }

    #[async_trait::async_trait]
    impl SttProvider for SuspendingProvider {
        fn id(&self) -> &ProviderId {
            &self.id
        }

        fn capabilities(&self) -> SttCapabilities {
            SttCapabilities::default()
        }

        async fn transcribe(
            &self,
            _audio: AudioBuffer,
            _opts: &SttOptions,
        ) -> Result<Transcript, SttError> {
            std::future::pending::<()>().await;
            unreachable!("pending() never resolves")
        }

        async fn health_check(&self) -> Result<HealthReport, SttError> {
            Ok(HealthReport {
                ok: true,
                latency_ms: None,
                detail: None,
            })
        }
    }

    fn audio() -> AudioBuffer {
        AudioBuffer::dictation(vec![0.0; 1600])
    }

    fn opts() -> SttOptions {
        SttOptions {
            language: Some("pt".to_string()),
            vocabulary_hints: Vec::new(),
            diarize: false,
            timeout: Duration::from_secs(15),
        }
    }

    fn transcript(text: &str) -> Transcript {
        Transcript {
            text: text.to_string(),
            provider_latency_ms: 7,
            ..Transcript::empty()
        }
    }

    fn provider_err(msg: &str) -> SttError {
        SttError::Provider(msg.to_string())
    }

    /// Backoff zero para os testes de rate limit não dormirem.
    fn zero_backoff() -> RetryPolicy {
        RetryPolicy {
            backoff_base: Duration::ZERO,
            backoff_cap: Duration::ZERO,
            ..RetryPolicy::default()
        }
    }

    #[tokio::test]
    async fn success_on_first_attempt() {
        let primary = MockProvider::new("local_whisper", vec![Ok(transcript("olá mundo"))]);
        let orchestrator = SttOrchestrator::new(primary.clone(), None);

        let outcome = orchestrator.transcribe(audio(), &opts()).await.unwrap();

        assert_eq!(outcome.transcript.text, "olá mundo");
        assert_eq!(outcome.provider_id, ProviderId::from("local_whisper"));
        assert_eq!(outcome.provider_attempts, 1);
        assert!(!outcome.used_fallback);
        assert_eq!(primary.calls(), 1);
    }

    #[tokio::test]
    async fn one_failure_retries_once_then_succeeds() {
        // FR-003-16 v1: falha do motor local → 1 retentativa.
        let primary = MockProvider::new(
            "local_whisper",
            vec![
                Err(provider_err("engine hiccup")),
                Ok(transcript("segunda tentativa")),
            ],
        );
        let orchestrator = SttOrchestrator::new(primary.clone(), None);

        let outcome = orchestrator.transcribe(audio(), &opts()).await.unwrap();

        assert_eq!(outcome.transcript.text, "segunda tentativa");
        assert_eq!(outcome.provider_attempts, 2);
        assert_eq!(primary.calls(), 2);
    }

    #[tokio::test]
    async fn two_failures_become_session_error() {
        // FR-003-16 v1: depois do retry, erro da sessão (o áudio é preservado
        // pela camada de sessão, que grava o WAV antes do resultado).
        let primary = MockProvider::new(
            "local_whisper",
            vec![
                Err(provider_err("first failure")),
                Err(provider_err("second failure")),
            ],
        );
        let orchestrator = SttOrchestrator::new(primary.clone(), None);

        let err = orchestrator.transcribe(audio(), &opts()).await.unwrap_err();

        assert!(matches!(err, SttError::Provider(msg) if msg == "second failure"));
        assert_eq!(primary.calls(), 2);
    }

    #[tokio::test]
    async fn model_not_ready_retries_once() {
        let primary = MockProvider::new(
            "local_whisper",
            vec![Err(SttError::ModelNotReady), Ok(transcript("reloaded"))],
        );
        let orchestrator = SttOrchestrator::new(primary.clone(), None);

        let outcome = orchestrator.transcribe(audio(), &opts()).await.unwrap();

        assert_eq!(outcome.transcript.text, "reloaded");
        assert_eq!(primary.calls(), 2);
    }

    #[tokio::test]
    async fn exhausted_recoverable_error_uses_fallback() {
        // AC-003-04 shape: primary falha além do retry → fallback produz o
        // texto e o outcome registra o provedor usado.
        let primary = MockProvider::new(
            "openai",
            vec![
                Err(SttError::Provider("503 upstream".to_string())),
                Err(SttError::Provider("503 upstream".to_string())),
            ],
        );
        let fallback = MockProvider::new("local_whisper", vec![Ok(transcript("offline"))]);
        let orchestrator = SttOrchestrator::new(
            primary.clone(),
            Some(fallback.clone() as Arc<dyn SttProvider>),
        );

        let outcome = orchestrator.transcribe(audio(), &opts()).await.unwrap();

        assert_eq!(outcome.transcript.text, "offline");
        assert_eq!(outcome.provider_id, ProviderId::from("local_whisper"));
        assert!(outcome.used_fallback);
        assert_eq!(primary.calls(), 2);
        assert_eq!(fallback.calls(), 1);
    }

    #[tokio::test]
    async fn offline_skips_retry_and_falls_back() {
        let primary = MockProvider::new("openai", vec![Err(SttError::Offline)]);
        let fallback = MockProvider::new("local_whisper", vec![Ok(transcript("local"))]);
        let orchestrator = SttOrchestrator::new(
            primary.clone(),
            Some(fallback.clone() as Arc<dyn SttProvider>),
        );

        let outcome = orchestrator.transcribe(audio(), &opts()).await.unwrap();

        assert!(outcome.used_fallback);
        assert_eq!(primary.calls(), 1);
        assert_eq!(fallback.calls(), 1);
    }

    #[tokio::test]
    async fn audio_too_large_falls_back_without_retry() {
        let primary = MockProvider::new("openai", vec![Err(SttError::AudioTooLarge)]);
        let fallback = MockProvider::new("local_whisper", vec![Ok(transcript("big audio"))]);
        let orchestrator = SttOrchestrator::new(
            primary.clone(),
            Some(fallback.clone() as Arc<dyn SttProvider>),
        );

        let outcome = orchestrator.transcribe(audio(), &opts()).await.unwrap();

        assert_eq!(outcome.transcript.text, "big audio");
        assert_eq!(primary.calls(), 1);
        assert_eq!(fallback.calls(), 1);
    }

    #[tokio::test]
    async fn auth_fails_immediately_without_fallback() {
        // 401/403 → erro direto: nem retry nem fallback (FR-003-16).
        let primary = MockProvider::new("openai", vec![Err(SttError::Auth)]);
        let fallback = MockProvider::new("local_whisper", vec![Ok(transcript("local"))]);
        let orchestrator = SttOrchestrator::new(
            primary.clone(),
            Some(fallback.clone() as Arc<dyn SttProvider>),
        );

        let err = orchestrator.transcribe(audio(), &opts()).await.unwrap_err();

        assert!(matches!(err, SttError::Auth));
        assert_eq!(primary.calls(), 1);
        assert_eq!(fallback.calls(), 0);
    }

    #[tokio::test]
    async fn rate_limited_retries_with_backoff_budget_then_falls_back() {
        // 429 → máx. 2 retentativas respeitando Retry-After, depois fallback.
        let primary = MockProvider::new(
            "groq",
            vec![
                Err(SttError::RateLimited {
                    retry_after: Some(Duration::ZERO),
                }),
                Err(SttError::RateLimited { retry_after: None }),
                Err(SttError::RateLimited { retry_after: None }),
            ],
        );
        let fallback = MockProvider::new("local_whisper", vec![Ok(transcript("local"))]);
        let mut orchestrator = SttOrchestrator::new(
            primary.clone(),
            Some(fallback.clone() as Arc<dyn SttProvider>),
        );
        orchestrator.policy = zero_backoff();

        let outcome = orchestrator.transcribe(audio(), &opts()).await.unwrap();

        assert!(outcome.used_fallback);
        assert_eq!(primary.calls(), 3); // 1 tentativa + 2 retentativas
        assert_eq!(fallback.calls(), 1);
    }

    #[test]
    fn blocking_driver_runs_inline_providers() {
        // O pipeline de ditado chama transcribe() de threads síncronas; o
        // motor local resolve no primeiro poll.
        let primary = MockProvider::new(
            "local_whisper",
            vec![Err(provider_err("hiccup")), Ok(transcript("ok"))],
        );
        let orchestrator = SttOrchestrator::new(primary.clone(), None);

        let outcome = orchestrator.transcribe_blocking(audio(), &opts()).unwrap();

        assert_eq!(outcome.transcript.text, "ok");
        assert_eq!(outcome.provider_attempts, 2);
    }

    #[test]
    fn blocking_driver_errors_instead_of_hanging_on_suspending_provider() {
        let primary = Arc::new(SuspendingProvider {
            id: ProviderId::from("cloud"),
        });
        let orchestrator = SttOrchestrator::new(primary, None);

        let err = orchestrator
            .transcribe_blocking(audio(), &opts())
            .unwrap_err();

        assert!(matches!(err, SttError::Provider(_)));
    }

    #[test]
    fn dictation_timeout_scales_with_audio_length() {
        // FR-003-15: 15 s + 1 s por 10 s de áudio.
        assert_eq!(
            SttOrchestrator::dictation_timeout(0.0),
            Duration::from_secs(15)
        );
        assert_eq!(
            SttOrchestrator::dictation_timeout(30.0),
            Duration::from_secs(18)
        );
        assert_eq!(
            SttOrchestrator::dictation_timeout(90.0),
            Duration::from_secs(24)
        );
    }

    #[test]
    fn error_classification_matches_fr_003_16() {
        assert!(SttError::Timeout.is_recoverable());
        assert!(SttError::Offline.is_recoverable());
        assert!(SttError::RateLimited { retry_after: None }.is_recoverable());
        assert!(!SttError::Auth.is_recoverable());
        assert!(!SttError::Auth.may_fallback());
        assert!(SttError::Provider("x".to_string()).may_fallback());
        assert!(SttError::AudioTooLarge.may_fallback());
    }
}
