//! `LocalSttProvider`: os motores locais (whisper.cpp via transcribe-cpp e os
//! modelos ONNX do transcribe-rs) vistos como [`SttProvider`].
//!
//! Um único provider cobre todos os motores locais na v1 porque a seleção de
//! modelo mora no `ModelManager`; quando a v1.1 trouxer "Modelos &
//! Provedores" (FR-003-01), cada configuração de modelo pode virar um provider.

use super::provider::SttProvider;
use super::types::{
    AudioBuffer, HealthReport, LanguageSupport, ProviderId, SttCapabilities, SttError, SttOptions,
    Transcript,
};
use crate::managers::model::{EngineType, ModelManager};
use crate::managers::transcription::TranscriptionManager;
use crate::settings::AppSettings;
use log::debug;
use std::sync::Arc;
use std::time::Instant;

/// IDs de tipo por motor — os nomes da tabela de provedores do F003.
const PROVIDER_WHISPER: &str = "local_whisper";
const PROVIDER_PARAKEET: &str = "local_parakeet";
const PROVIDER_ONNX: &str = "local_onnx";
const PROVIDER_LOCAL: &str = "local";

/// O motor local carregado como provedor STT. Segura o `TranscriptionManager`
/// (dono do ciclo de vida do motor: carga, lease, idle-unload, streaming) e o
/// `ModelManager` (metadados do modelo, para `capabilities()`).
///
/// Construído por transcrição — barato: só `Arc` clones. Não pode ser guardado
/// dentro do próprio `TranscriptionManager`: o `Drop` do manager usa a contagem
/// de `Arc`s para decidir quando parar a thread de idle-unload, e um provider
/// interno a segurando vazaria a thread.
pub struct LocalSttProvider {
    manager: TranscriptionManager,
    model_manager: Arc<ModelManager>,
    id: ProviderId,
    session_settings: Option<AppSettings>,
}

impl LocalSttProvider {
    pub fn new(manager: TranscriptionManager, model_manager: Arc<ModelManager>) -> Self {
        Self::for_session(manager, model_manager, None)
    }

    pub(crate) fn for_session(
        manager: TranscriptionManager,
        model_manager: Arc<ModelManager>,
        session_settings: Option<AppSettings>,
    ) -> Self {
        let id = Self::provider_id_for(&manager, &model_manager);
        Self {
            manager,
            model_manager,
            id,
            session_settings,
        }
    }

    /// `local_whisper` para modelos transcribe-cpp (família whisper e demais
    /// archs GGUF), `local_parakeet` para Parakeet/ONNX, `local_onnx` para os
    /// demais motores ONNX; `local` quando nenhum modelo está carregado.
    fn provider_id_for(manager: &TranscriptionManager, model_manager: &ModelManager) -> ProviderId {
        let id = manager
            .get_current_model()
            .and_then(|model_id| model_manager.get_model_info(&model_id))
            .map(|info| match info.engine_type {
                EngineType::TranscribeCpp => PROVIDER_WHISPER,
                EngineType::Parakeet => PROVIDER_PARAKEET,
                _ => PROVIDER_ONNX,
            })
            .unwrap_or(PROVIDER_LOCAL);
        ProviderId::new(id)
    }
}

#[async_trait::async_trait]
impl SttProvider for LocalSttProvider {
    fn id(&self) -> &ProviderId {
        &self.id
    }

    fn capabilities(&self) -> SttCapabilities {
        let info = self
            .manager
            .get_current_model()
            .and_then(|model_id| self.model_manager.get_model_info(&model_id));

        let (supports_prompt, auto_detect, languages) = match &info {
            Some(info) => (
                // Só a família whisper aceita `initial_prompt` (FR-003-12);
                // os demais motores recebem os termos via correção fuzzy.
                matches!(info.engine_type, EngineType::TranscribeCpp),
                info.supports_language_detection,
                info.supported_languages.clone(),
            ),
            None => (false, true, Vec::new()),
        };

        SttCapabilities {
            local: true,
            max_upload_bytes: None,
            max_audio_secs: None,
            supports_prompt,
            supports_diarization: false,
            // A v1 não emite segmentos com timestamp (FR-003-15).
            supports_segments: false,
            languages: LanguageSupport {
                auto_detect,
                languages,
            },
        }
    }

    async fn transcribe(
        &self,
        audio: AudioBuffer,
        opts: &SttOptions,
    ) -> Result<Transcript, SttError> {
        debug!(
            "STT request via '{}': {:.2}s of audio captured {:.2}s ago (diarize={})",
            self.id,
            audio.duration_secs(),
            audio.started_at.elapsed().as_secs_f64(),
            opts.diarize
        );
        // Os motores locais são síncronos: executa inline e o future resolve
        // no primeiro poll — é o que permite `transcribe_blocking` no pipeline
        // de ditado.
        match self.session_settings.as_ref() {
            Some(settings) => {
                self.manager
                    .transcribe_once_with_settings(&audio.samples, opts, Some(settings))
            }
            None => self.manager.transcribe_once(&audio.samples, opts),
        }
    }

    /// Na v1 o "modelo com áudio curto embutido" do contrato vira uma checagem
    /// de estado: modelo carregado e pronto. Não roda inferência de verdade —
    /// o custo de um run só para health check não se justifica localmente.
    #[allow(dead_code)] // chamado pelo command providers_test da v1.1
    async fn health_check(&self) -> Result<HealthReport, SttError> {
        let start = Instant::now();
        let latency_ms = Some(start.elapsed().as_millis() as u64);
        match self.manager.get_current_model() {
            Some(model) if self.manager.is_model_loaded() => Ok(HealthReport {
                ok: true,
                latency_ms,
                detail: Some(format!("local model '{model}' loaded")),
            }),
            Some(model) => Ok(HealthReport {
                ok: false,
                latency_ms,
                detail: Some(format!("local model '{model}' selected but not loaded")),
            }),
            None => Err(SttError::ModelNotReady),
        }
    }
}
