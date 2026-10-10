//! `TranscriptionManager`: dono do ciclo de vida do motor de STT local
//! (carga/descarga, idle-unload, lease para streaming) e porta de entrada do
//! pipeline de transcrição.
//!
//! A fronteira provedor/orquestrador vive em `crate::stt`:
//! [`stt::local::LocalSttProvider`] adapta este manager ao trait
//! [`stt::provider::SttProvider`] (contracts §2) e
//! [`stt::orchestrator::SttOrchestrator`] aplica retry único + fallback
//! (FR-003-16). Os subdomínios internos ficam nos módulos filhos:
//!
//! * [`engine`] — `LoadedEngine`, carga de modelo e seleção de
//!   backend/dispositivo de computação;
//! * [`streaming`] — transcrição ao vivo por frames + `StreamRouter`;
//! * [`inference`] — `transcribe_once`: uma tentativa batch no motor;
//! * [`language`] — coerção de idioma e evidência do idioma de saída;
//! * [`prompt`] — montagem do `initial_prompt` do whisper (FR-003-12);
//! * [`hallucination`] — filtro de texto-fantasma do decoder (FR-003-13);
//! * [`postprocess`] — correção fuzzy, filler words e normalização de texto.

mod engine;
mod hallucination;
mod inference;
mod language;
mod postprocess;
mod prompt;
mod streaming;
pub(crate) mod translation;

pub use engine::{
    apply_accelerator_settings, describe_compute_devices, get_available_accelerators,
    init_transcribe_backend, report_compute_devices, AvailableAccelerators,
};
pub use streaming::{StreamPhaseEvent, StreamRouter, StreamTextEvent, StreamWorkKind};

use crate::managers::audio::AudioRecordingManager;
use crate::managers::model::ModelManager;
use crate::settings::{get_settings, AppSettings, ModelUnloadTimeout};
use crate::stt::local::LocalSttProvider;
use crate::stt::orchestrator::SttOrchestrator;
use crate::stt::types::{AudioBuffer, SttError, SttOptions};
use anyhow::Result;
use log::{debug, error, info, warn};
use serde::Serialize;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, SystemTime};
use tauri::{AppHandle, Emitter, Manager};

/// Mensagem extraída de um payload de pânico capturado por `catch_unwind`
/// (motor de inferência e pós-processamento de texto). Privada ao módulo —
/// os submódulos filhos a enxergam por `super::`.
fn panic_payload_message(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(message) = payload.downcast_ref::<&str>() {
        (*message).to_string()
    } else if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else {
        "unknown panic".to_string()
    }
}

/// Real-time factor: `audio_secs / compute_secs` — 4.0 = transcreveu 4x mais
/// rápido que o tempo real. 0 quando não houve compute medido.
fn real_time_factor(audio_secs: f64, compute_secs: f64) -> f64 {
    if compute_secs > 0.0 {
        audio_secs / compute_secs
    } else {
        0.0
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct ModelStateEvent {
    pub event_type: String,
    pub model_id: Option<String>,
    pub model_name: Option<String>,
    pub error: Option<String>,
}

/// RAII guard that clears the `is_loading` flag and notifies waiters on drop.
/// Ensures the loading flag is always reset, even on early returns or panics.
pub struct LoadingGuard {
    is_loading: Arc<Mutex<bool>>,
    loading_condvar: Arc<Condvar>,
    loading_model_id: Arc<Mutex<Option<String>>>,
}

impl Drop for LoadingGuard {
    fn drop(&mut self) {
        // Recover from a poisoned mutex instead of panicking —
        // a panic inside Drop calls abort().
        let mut is_loading = match self.is_loading.lock() {
            Ok(g) => g,
            Err(e) => {
                warn!("Recovered poisoned is_loading mutex during LoadingGuard drop — a panic occurred earlier this session");
                e.into_inner()
            }
        };
        *self
            .loading_model_id
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = None;
        *is_loading = false;
        self.loading_condvar.notify_all();
    }
}

/// Outcome of [`TranscriptionManager::stage_model_load_for`].
pub(crate) enum StagedModelLoad {
    /// The requested model is already loaded; nothing to run.
    Loaded,
    /// A load for this same model is already in flight.
    InFlight,
    /// The load is marked pending; run the job on a worker thread to perform
    /// it. Dropping the job un-marks the load and wakes condvar waiters.
    Pending(Box<dyn FnOnce() + Send>),
}

#[derive(Clone)]
pub struct TranscriptionManager {
    engine: Arc<Mutex<Option<engine::LoadedEngine>>>,
    model_manager: Arc<ModelManager>,
    app_handle: AppHandle,
    current_model_id: Arc<Mutex<Option<String>>>,
    last_activity: Arc<AtomicU64>,
    shutdown_signal: Arc<AtomicBool>,
    watcher_handle: Arc<Mutex<Option<thread::JoinHandle<()>>>>,
    is_loading: Arc<Mutex<bool>>,
    loading_condvar: Arc<Condvar>,
    loading_model_id: Arc<Mutex<Option<String>>>,
    reload_model_on_next_use: Arc<AtomicBool>,
    /// Routes real-time audio frames to the active streaming worker; see
    /// [`StreamRouter`]. Shared with the audio recorder so per-frame feeds skip
    /// Tauri state and the manager lock.
    router: Arc<StreamRouter>,
    /// True only while a transcribe-cpp `Stream` is actually in flight (set by
    /// the worker once `stream()` succeeds). Used for overlay/UI decisions.
    stream_active: Arc<AtomicBool>,
    /// Streaming uses four independent flags: router open = frames should route,
    /// worker active = no second worker may start, engine lease = engine is out
    /// of the mutex, stream active = UI should show a live session.
    ///
    /// Monotonic id source for stream workers; zero means "no worker".
    next_stream_worker_id: Arc<AtomicU64>,
    /// Nonzero while a stream worker exists, even if it has not leased the engine
    /// yet. This prevents a second worker from starting after finalize/cancel
    /// closes the router but before the first worker has fully exited.
    active_stream_worker: Arc<AtomicU64>,
    /// Nonzero while the streaming worker has taken the engine out of `engine`.
    /// `is_model_loaded()` consults this so the model still reports "loaded"
    /// while the worker holds it.
    active_engine_lease: Arc<AtomicU64>,
}

impl TranscriptionManager {
    pub fn new(app_handle: &AppHandle, model_manager: Arc<ModelManager>) -> Result<Self> {
        let manager = Self {
            engine: Arc::new(Mutex::new(None)),
            model_manager,
            app_handle: app_handle.clone(),
            current_model_id: Arc::new(Mutex::new(None)),
            last_activity: Arc::new(AtomicU64::new(Self::now_ms())),
            shutdown_signal: Arc::new(AtomicBool::new(false)),
            watcher_handle: Arc::new(Mutex::new(None)),
            is_loading: Arc::new(Mutex::new(false)),
            loading_condvar: Arc::new(Condvar::new()),
            loading_model_id: Arc::new(Mutex::new(None)),
            reload_model_on_next_use: Arc::new(AtomicBool::new(false)),
            router: Arc::new(StreamRouter::new()),
            stream_active: Arc::new(AtomicBool::new(false)),
            next_stream_worker_id: Arc::new(AtomicU64::new(1)),
            active_stream_worker: Arc::new(AtomicU64::new(0)),
            active_engine_lease: Arc::new(AtomicU64::new(0)),
        };

        // Start the idle watcher
        {
            let app_handle_cloned = app_handle.clone();
            let manager_cloned = manager.clone();
            let shutdown_signal = manager.shutdown_signal.clone();
            let handle = thread::spawn(move || {
                debug!("Idle watcher thread started");
                while !shutdown_signal.load(Ordering::Relaxed) {
                    thread::sleep(Duration::from_secs(10)); // Check every 10 seconds

                    // Check shutdown signal again after sleep
                    if shutdown_signal.load(Ordering::Relaxed) {
                        break;
                    }

                    let settings = get_settings(&app_handle_cloned);
                    let timeout = settings.model_unload_timeout;

                    // Skip Immediately — that variant is handled by
                    // maybe_unload_immediately() after each transcription.
                    // Treating it as 0s here would unload the model mid-recording.
                    if timeout == ModelUnloadTimeout::Immediately {
                        continue;
                    }

                    // While recording, keep the idle timer fresh so the
                    // model is never unloaded mid-session. A meeting counts
                    // too: its live transcriber leases the engine outside
                    // the dictation recorder, so a long meeting with sparse
                    // speech must not idle-unload the model mid-session
                    // (the very ModelNotReady storm this watcher would cause).
                    let is_recording = app_handle_cloned
                        .try_state::<Arc<AudioRecordingManager>>()
                        .is_some_and(|a| a.is_recording());
                    let meeting_active = crate::meeting::session::meeting_recording_active();
                    if is_recording || meeting_active {
                        manager_cloned.touch_activity();
                        continue;
                    }

                    if let Some(limit_seconds) = timeout.to_seconds() {
                        let last = manager_cloned.last_activity.load(Ordering::Relaxed);
                        let now_ms = TranscriptionManager::now_ms();
                        let idle_ms = now_ms.saturating_sub(last);
                        let limit_ms = limit_seconds * 1000;

                        if idle_ms > limit_ms {
                            // idle -> unload
                            if manager_cloned.is_model_loaded() {
                                let unload_start = std::time::Instant::now();
                                info!(
                                    "Model idle for {}s (limit: {}s), unloading",
                                    idle_ms / 1000,
                                    limit_seconds
                                );
                                match manager_cloned.unload_model() {
                                    Ok(()) => {
                                        let unload_duration = unload_start.elapsed();
                                        info!(
                                            "Model unloaded due to inactivity (took {}ms)",
                                            unload_duration.as_millis()
                                        );
                                    }
                                    Err(e) => {
                                        error!("Failed to unload idle model: {}", e);
                                    }
                                }
                            }
                        }
                    }
                }
                debug!("Idle watcher thread shutting down gracefully");
            });
            *manager.watcher_handle.lock().unwrap() = Some(handle);
        }

        Ok(manager)
    }

    /// Lock the engine mutex, recovering from poison if a previous transcription panicked.
    fn lock_engine(&self) -> MutexGuard<'_, Option<engine::LoadedEngine>> {
        self.engine.lock().unwrap_or_else(|poisoned| {
            warn!("Engine mutex was poisoned by a previous panic, recovering");
            poisoned.into_inner()
        })
    }

    pub fn is_model_loaded(&self) -> bool {
        // The engine may be leased out to the streaming worker (taken out of
        // the mutex). It's still loaded, just in use, so report true.
        self.lock_engine().is_some() || self.active_engine_lease.load(Ordering::Acquire) != 0
    }

    /// Accelerator changes should not disturb the current transcription. Mark
    /// the cached engine stale; the next model-use path reloads it with the
    /// latest settings.
    pub fn reload_model_on_next_use(&self) {
        self.reload_model_on_next_use.store(true, Ordering::Release);
    }

    /// Atomically check whether a model load is in progress and, if not, mark
    /// one as starting. Returns a [`LoadingGuard`] whose [`Drop`] impl will
    /// clear the flag and wake waiters. Returns `None` if a load is already in
    /// progress.
    pub fn try_start_loading_for(&self, model_id: &str) -> Option<LoadingGuard> {
        let mut is_loading = self.is_loading.lock().unwrap();
        if *is_loading {
            return None;
        }
        *is_loading = true;
        *self
            .loading_model_id
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = Some(model_id.to_string());
        Some(LoadingGuard {
            is_loading: self.is_loading.clone(),
            loading_condvar: self.loading_condvar.clone(),
            loading_model_id: self.loading_model_id.clone(),
        })
    }

    pub fn unload_model(&self) -> Result<()> {
        let unload_start = std::time::Instant::now();
        debug!("Starting to unload model");

        {
            let mut engine = self.lock_engine();
            // Dropping the engine frees all resources
            *engine = None;
        }
        {
            let mut current_model = self.current_model_id.lock().unwrap();
            *current_model = None;
        }

        // Emit unloaded event
        let _ = self.app_handle.emit(
            "model-state-changed",
            ModelStateEvent {
                event_type: "unloaded".to_string(),
                model_id: None,
                model_name: None,
                error: None,
            },
        );

        let unload_duration = unload_start.elapsed();
        debug!(
            "Model unloaded manually (took {}ms)",
            unload_duration.as_millis()
        );
        Ok(())
    }

    fn now_ms() -> u64 {
        SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64
    }

    /// Reset the idle timer to now.
    fn touch_activity(&self) {
        self.last_activity.store(Self::now_ms(), Ordering::Relaxed);
    }

    /// Unloads the model immediately if the setting is enabled and the model is loaded
    pub fn maybe_unload_immediately(&self, context: &str) {
        let settings = get_settings(&self.app_handle);
        if settings.model_unload_timeout == ModelUnloadTimeout::Immediately
            && self.is_model_loaded()
        {
            info!("Immediately unloading model after {}", context);
            if let Err(e) = self.unload_model() {
                warn!("Failed to immediately unload model: {}", e);
            }
        }
    }

    /// Kicks off the model loading in a background thread if it's not already loaded
    pub fn initiate_model_load(&self) {
        self.initiate_model_load_for(&get_settings(&self.app_handle).selected_model);
    }

    /// Load a session-selected model without modifying the persisted selection.
    pub(crate) fn initiate_model_load_for(&self, model_id: &str) -> bool {
        match self.stage_model_load_for(model_id) {
            Some(StagedModelLoad::Pending(run)) => {
                thread::spawn(run);
                true
            }
            Some(StagedModelLoad::Loaded | StagedModelLoad::InFlight) => true,
            None => false,
        }
    }

    /// Mark a session model load as pending and hand back the load job without
    /// running it, so the caller can interleave latency-critical work (opening
    /// the microphone) before the heavy load starts competing for CPU. Audio
    /// spoken while the mic stream is still opening is unrecoverable, whereas
    /// the model can keep loading under held speech — the inference path waits
    /// on the load condvar either way.
    ///
    /// `is_loading` is set immediately, so `start_stream`/`transcribe` waiters
    /// already see the pending load. Dropping the [`StagedModelLoad::Pending`]
    /// job without running it releases the mark and wakes waiters.
    pub(crate) fn stage_model_load_for(&self, model_id: &str) -> Option<StagedModelLoad> {
        let mut is_loading = self.is_loading.lock().unwrap();
        if *is_loading {
            // A cold start may capture while its own model loads, as before.
            // A different/unknown in-flight selection must fail closed.
            return loading_model_matches_request(
                self.loading_model_id
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .as_deref(),
                model_id,
            )
            .then_some(StagedModelLoad::InFlight);
        }

        let reload_pending = self.reload_model_on_next_use.load(Ordering::Acquire);
        if !reload_pending
            && self.is_model_loaded()
            && self.get_current_model().as_deref() == Some(model_id)
        {
            return Some(StagedModelLoad::Loaded);
        }

        *is_loading = true;
        *self
            .loading_model_id
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = Some(model_id.to_string());
        // The guard is created now (not inside the job) so that dropping a
        // never-run `Pending` still releases the `is_loading` mark.
        let loading_guard = LoadingGuard {
            is_loading: self.is_loading.clone(),
            loading_condvar: self.loading_condvar.clone(),
            loading_model_id: self.loading_model_id.clone(),
        };
        let self_clone = self.clone();
        let model_id = model_id.to_string();
        Some(StagedModelLoad::Pending(Box::new(move || {
            let _loading_guard = loading_guard;
            if reload_pending {
                self_clone
                    .reload_model_on_next_use
                    .store(false, Ordering::Release);
            }
            if let Err(e) = self_clone.load_model(&model_id) {
                error!("Failed to load model: {}", e);
            }
        })))
    }

    /// Whether a model load is currently in progress.
    pub fn is_loading_model(&self) -> bool {
        *self.is_loading.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn get_current_model(&self) -> Option<String> {
        let current_model = self.current_model_id.lock().unwrap();
        current_model.clone()
    }

    /// Transcreve `audio` (PCM f32 mono 16 kHz) passando pelo orquestrador de
    /// provedores: seleção → retry único → fallback (FR-003-16 / contracts §2).
    ///
    /// Em v1 o único provedor é o motor local ([`LocalSttProvider`]) e não há
    /// fallback configurado; o desfecho é "1 retentativa, depois erro da sessão
    /// com áudio preservado" (a camada de sessão já grava o WAV em paralelo).
    /// A assinatura `Result<String>` e o fluxo ditado→transcrição→histórico
    /// permanecem idênticos.
    pub fn transcribe(&self, audio: Vec<f32>) -> Result<String> {
        self.transcribe_with_settings(audio, None)
    }

    /// A private snapshot carries translation only for this session/provider.
    pub(crate) fn transcribe_with_settings(
        &self,
        audio: Vec<f32>,
        session_settings: Option<AppSettings>,
    ) -> Result<String> {
        self.touch_activity();
        let settings = session_settings
            .clone()
            .unwrap_or_else(|| get_settings(&self.app_handle));
        let buffer = AudioBuffer::dictation(audio);
        let opts = SttOptions {
            // `None` = auto, conforme o contrato.
            language: (settings.selected_language != "auto")
                .then(|| settings.selected_language.clone()),
            vocabulary_hints: settings.custom_words.clone(),
            diarize: false,
            timeout: SttOrchestrator::dictation_timeout(buffer.duration_secs()),
        };
        let orchestrator = SttOrchestrator::new(
            Arc::new(LocalSttProvider::for_session(
                self.clone(),
                Arc::clone(&self.model_manager),
                session_settings,
            )),
            None,
        );

        orchestrator
            .transcribe_blocking(buffer, &opts)
            .map(|outcome| {
                if outcome.used_fallback || outcome.provider_attempts > 1 {
                    info!(
                        "Transcription completed by provider '{}' after {} attempt(s) (fallback={})",
                        outcome.provider_id, outcome.provider_attempts, outcome.used_fallback
                    );
                }
                outcome.transcript.text
            })
            .map_err(SttError::into_anyhow)
    }

    /// Return the leased engine to the mutex, unless the model was switched or
    /// unloaded during transcription (in which case the stale engine is dropped).
    fn return_engine(&self, engine: engine::LoadedEngine, expected_model_id: &str) {
        let still_current =
            self.current_model_id.lock().unwrap().as_deref() == Some(expected_model_id);
        if still_current {
            *self.lock_engine() = Some(engine);
        } else {
            info!(
                "Model changed/unloaded during transcription; dropping stale engine (was '{}')",
                expected_model_id
            );
            // `engine` drops here, freeing its resources.
        }
    }
}

impl Drop for TranscriptionManager {
    fn drop(&mut self) {
        // Skip shutdown unless this is the very last clone. TranscriptionManager
        // is cloned by initiate_model_load() and the watcher thread — those
        // clones dropping must not kill the watcher. The watcher thread holds
        // its own clone, so engine's strong_count is always >= 2 while the
        // watcher is alive. When it reaches 1, only this instance remains
        // and we can safely shut down.
        if Arc::strong_count(&self.engine) > 1 {
            return;
        }

        // Signal the watcher thread to shutdown
        self.shutdown_signal.store(true, Ordering::Relaxed);

        // Wait for the thread to finish gracefully.
        // Use match instead of unwrap to avoid panicking if the mutex is
        // poisoned — a panic inside Drop calls abort().
        let mut guard = match self.watcher_handle.lock() {
            Ok(g) => g,
            Err(e) => {
                warn!("Recovered poisoned watcher_handle mutex during TranscriptionManager drop — a panic occurred earlier this session");
                e.into_inner()
            }
        };
        if let Some(handle) = guard.take() {
            if let Err(e) = handle.join() {
                warn!("Failed to join idle watcher thread: {:?}", e);
            } else {
                debug!("Idle watcher thread joined successfully");
            }
        }
    }
}

fn loading_model_matches_request(pending: Option<&str>, requested: &str) -> bool {
    pending == Some(requested)
}

#[cfg(test)]
mod translation_load_tests {
    #[test]
    fn translation_load_matching_preserves_normal_cold_start() {
        assert!(super::loading_model_matches_request(Some("turbo"), "turbo"));
        assert!(!super::loading_model_matches_request(
            Some("turbo"),
            "medium"
        ));
        assert!(!super::loading_model_matches_request(
            Some("medium"),
            "turbo"
        ));
        assert!(!super::loading_model_matches_request(None, "medium"));
    }
}
