//! Uma tentativa de transcrição batch no motor carregado
//! ([`TranscriptionManager::transcribe_once`]): resolve idioma/prompt contra as
//! capacidades reais do modelo, executa o dispatch por backend sob
//! `catch_unwind` e aplica o pós-processamento de texto.
//!
//! A política de retry/fallback **não** mora aqui — é do
//! `crate::stt::orchestrator` (FR-003-16). Este método é a implementação de
//! `SttProvider::transcribe` do `LocalSttProvider`.

use super::engine::LoadedEngine;
use super::language::{
    effective_language_for_model, normalize_cjk_language, resolve_output_language_evidence,
    transcribe_cpp_run_plan, with_model_detected_language,
};
use super::postprocess::post_process_transcription_text;
use super::{panic_payload_message, real_time_factor, ModelStateEvent, TranscriptionManager};
use crate::settings::get_settings;
use crate::stt::types::{SttError, SttOptions, Transcript};
use log::{debug, error, info};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::time::Instant;
use tauri::Emitter;
use transcribe_cpp::{Feature, RunExtension, RunOptions, WhisperRunOptions};
use transcribe_rs::{
    onnx::{
        parakeet::{ParakeetParams, TimestampGranularity},
        sense_voice::SenseVoiceParams,
    },
    SpeechModel, TranscribeOptions,
};

impl TranscriptionManager {
    /// Uma tentativa de transcrição no motor local carregado.
    ///
    /// Chamada por `LocalSttProvider` (que é dirigido pelo orquestrador) —
    /// cada chamada é exatamente uma tentativa: erros viram [`SttError`]
    /// tipados e a decisão de repetir/cair no fallback fica fora daqui.
    pub(crate) fn transcribe_once(
        &self,
        audio: &[f32],
        opts: &SttOptions,
    ) -> Result<Transcript, SttError> {
        #[cfg(debug_assertions)]
        if std::env::var("TRANSCREVE_FORCE_TRANSCRIPTION_FAILURE").is_ok() {
            return Err(SttError::Provider(
                "Simulated transcription failure (TRANSCREVE_FORCE_TRANSCRIPTION_FAILURE)"
                    .to_string(),
            ));
        }

        // Update last activity timestamp
        self.touch_activity();

        let st = std::time::Instant::now();
        let audio_len = audio.len();

        debug!("Audio vector length: {}", audio_len);

        if audio.is_empty() {
            debug!("Empty audio vector");
            self.maybe_unload_immediately("empty audio");
            return Ok(Transcript::empty());
        }

        // Check if model is loaded, if not try to load it
        {
            // If the model is loading, wait for it to complete.
            let mut is_loading = self.is_loading.lock().unwrap();
            while *is_loading {
                is_loading = self.loading_condvar.wait(is_loading).unwrap();
            }

            let engine_guard = self.lock_engine();
            if engine_guard.is_none() {
                return Err(SttError::ModelNotReady);
            }
        }

        // Get current settings for configuration
        let settings = get_settings(&self.app_handle);

        // Validate selected language against the model's supported languages.
        // If the language isn't supported, fall back to "auto" to prevent errors.
        // Validate against the model that's actually loaded (which can differ
        // from settings.selected_model when a caller loaded a specific model —
        // e.g. the --transcribe-file path's --model), not the persisted
        // selection.
        let active_model = self
            .get_current_model()
            .unwrap_or_else(|| settings.selected_model.clone());
        // A intenção de idioma chega em `opts` (contrato `SttProvider`); a
        // coerção é capability-aware — um modelo que exige escolha nunca recebe
        // "auto" — e nunca volta para as settings, então a intenção sobrevive a
        // trocas de modelo.
        let language_intent = opts.language.clone().unwrap_or_else(|| "auto".to_string());
        let validated_language = effective_language_for_model(
            &language_intent,
            self.model_manager.as_ref(),
            &active_model,
        );
        if validated_language != language_intent {
            debug!(
                "Language intent '{}' resolved to '{}' for model '{}'",
                language_intent, validated_language, active_model
            );
        }

        // Whether the loaded model is actually whisper-family (arch string).
        // Non-whisper archs (e.g. Voxtral Small) can advertise
        // Feature::InitialPrompt yet reject the whisper-kind run extension
        // with INVALID_ARG, so the whisper extension must be gated on the
        // arch, not on the feature (see #1601).
        let mut model_is_whisper = false;

        // Perform transcription with the appropriate engine.
        // We use catch_unwind to prevent engine panics from poisoning the mutex,
        // which would make the app hang indefinitely on subsequent operations.
        let engine_start = Instant::now();
        let (result, output_language, model_languages) = {
            let mut engine_guard = self.lock_engine();

            // Take the engine out so we own it during transcription.
            // If the engine panics, we simply don't put it back (effectively unloading it)
            // instead of poisoning the mutex.
            let mut engine = match engine_guard.take() {
                Some(e) => e,
                None => {
                    return Err(SttError::Provider(
                        "Model failed to load after auto-load attempt. \
                         Please check your model settings."
                            .to_string(),
                    ));
                }
            };

            // Release the lock before transcribing — no mutex held during the engine call
            drop(engine_guard);

            // Probe live transcribe-cpp capabilities once (cheap GGUF-metadata
            // reads); the loaded session is the source of truth, not the
            // ModelManager copy. The whisper run extension is kind-tagged, so
            // non-whisper archs (parakeet, voxtral, …) reject it with
            // INVALID_ARG; attach it — and translate — only where supported.
            let mut model_supports_translate = false;
            let mut model_languages = self
                .model_manager
                .get_model_info(&active_model)
                .map(|info| info.supported_languages)
                .unwrap_or_default();
            let mut output_was_translated = false;
            let mut applied_language_hint: Option<String> = None;
            let mut model_detected_language: Option<String> = None;
            if let LoadedEngine::TranscribeCpp(session) = &engine {
                let model = session.model();
                let caps = model.capabilities();
                // Whether the loaded transcribe-cpp model advertises
                // Feature::InitialPrompt. Informational (logged below); the whisper
                // run extension and the fuzzy-correction skip are gated on
                // `model_is_whisper` instead, since non-whisper archs can advertise
                // the feature while rejecting the whisper-kind extension.
                let model_takes_initial_prompt = model.supports(Feature::InitialPrompt);
                model_is_whisper = model.arch() == "whisper";
                model_supports_translate = caps.supports_translate;
                model_languages = caps.languages;
                debug!(
                    "transcribe-cpp model '{}' on '{}': initial_prompt={}, translate={}, languages={:?}",
                    settings.selected_model,
                    model.backend(),
                    model_takes_initial_prompt,
                    model_supports_translate,
                    model_languages
                );
            }

            let transcribe_result =
                catch_unwind(AssertUnwindSafe(|| -> Result<String, SttError> {
                    match &mut engine {
                        LoadedEngine::TranscribeCpp(session) => {
                            // Custom words become the initial prompt ONLY for models
                            // that accept one (whisper family). Attaching the
                            // whisper run extension to a non-whisper arch is rejected
                            // with INVALID_ARG, so skip it there and let the fuzzy
                            // post-correction handle custom words instead.
                            let family = if opts.vocabulary_hints.is_empty() || !model_is_whisper {
                                None
                            } else {
                                Some(RunExtension::Whisper(WhisperRunOptions {
                                    initial_prompt: Some(opts.vocabulary_hints.join(", ")),
                                    ..Default::default()
                                }))
                            };

                            let run_plan = transcribe_cpp_run_plan(
                                settings.translate_to_english,
                                &validated_language,
                                &model_languages,
                                model_supports_translate,
                            );
                            output_was_translated =
                                run_plan.target_language.as_deref() == Some("en");
                            applied_language_hint = run_plan.language.clone();

                            let run_options = RunOptions {
                                task: run_plan.task,
                                language: run_plan.language,
                                target_language: run_plan.target_language,
                                family,
                                ..Default::default()
                            };

                            debug!(
                                "transcribe-cpp run: task={:?}, language={:?}, initial_prompt={}",
                                run_options.task,
                                run_options.language,
                                run_options.family.is_some()
                            );

                            session
                                .run(audio, &run_options)
                                .map(|t| {
                                    // Whisper's audio-based LID (auto mode only;
                                    // `None` when a language hint was passed).
                                    model_detected_language = t.language;
                                    t.text
                                })
                                .map_err(|e| {
                                    SttError::Provider(format!(
                                        "transcribe-cpp transcription failed: {}",
                                        e
                                    ))
                                })
                        }
                        LoadedEngine::Parakeet(parakeet_engine) => {
                            let params = ParakeetParams {
                                timestamp_granularity: Some(TimestampGranularity::Segment),
                                ..Default::default()
                            };
                            parakeet_engine
                                .transcribe_with(audio, &params)
                                .map(|r| r.text)
                                .map_err(|e| {
                                    SttError::Provider(format!(
                                        "Parakeet transcription failed: {}",
                                        e
                                    ))
                                })
                        }
                        LoadedEngine::Moonshine(moonshine_engine) => moonshine_engine
                            .transcribe(audio, &TranscribeOptions::default())
                            .map(|r| r.text)
                            .map_err(|e| {
                                SttError::Provider(format!("Moonshine transcription failed: {}", e))
                            }),
                        LoadedEngine::MoonshineStreaming(streaming_engine) => streaming_engine
                            .transcribe(audio, &TranscribeOptions::default())
                            .map(|r| r.text)
                            .map_err(|e| {
                                SttError::Provider(format!(
                                    "Moonshine streaming transcription failed: {}",
                                    e
                                ))
                            }),
                        LoadedEngine::SenseVoice(sense_voice_engine) => {
                            let language = match normalize_cjk_language(&validated_language) {
                                "zh" => Some("zh".to_string()),
                                "en" => Some("en".to_string()),
                                "ja" => Some("ja".to_string()),
                                "ko" => Some("ko".to_string()),
                                "yue" => Some("yue".to_string()),
                                _ => None,
                            };
                            applied_language_hint = language.clone();
                            let params = SenseVoiceParams {
                                language,
                                use_itn: Some(true),
                            };
                            sense_voice_engine
                                .transcribe_with(audio, &params)
                                .map(|r| r.text)
                                .map_err(|e| {
                                    SttError::Provider(format!(
                                        "SenseVoice transcription failed: {}",
                                        e
                                    ))
                                })
                        }
                        LoadedEngine::GigaAM(gigaam_engine) => gigaam_engine
                            .transcribe(audio, &TranscribeOptions::default())
                            .map(|r| r.text)
                            .map_err(|e| {
                                SttError::Provider(format!("GigaAM transcription failed: {}", e))
                            }),
                        LoadedEngine::Canary(canary_engine) => {
                            output_was_translated = settings.translate_to_english;
                            let lang = if validated_language == "auto" {
                                None
                            } else {
                                Some(validated_language.clone())
                            };
                            applied_language_hint = lang.clone();
                            let options = TranscribeOptions {
                                language: lang,
                                translate: settings.translate_to_english,
                                ..Default::default()
                            };
                            canary_engine
                                .transcribe(audio, &options)
                                .map(|r| r.text)
                                .map_err(|e| {
                                    SttError::Provider(format!(
                                        "Canary transcription failed: {}",
                                        e
                                    ))
                                })
                        }
                        LoadedEngine::Cohere(cohere_engine) => {
                            let lang = if validated_language == "auto" {
                                None
                            } else {
                                Some(normalize_cjk_language(&validated_language).to_string())
                            };
                            applied_language_hint = lang.clone();
                            let options = TranscribeOptions {
                                language: lang,
                                ..Default::default()
                            };
                            cohere_engine
                                .transcribe(audio, &options)
                                .map(|r| r.text)
                                .map_err(|e| {
                                    SttError::Provider(format!(
                                        "Cohere transcription failed: {}",
                                        e
                                    ))
                                })
                        }
                    }
                }));

            let text = match transcribe_result {
                Ok(inner_result) => {
                    // Success or normal error: return the engine unless a model
                    // switch/unload invalidated it while it was in use.
                    self.return_engine(engine, &active_model);
                    inner_result?
                }
                Err(panic_payload) => {
                    // Engine panicked — do NOT put it back (it's in an unknown state).
                    // The engine is dropped here, effectively unloading it.
                    let panic_msg = panic_payload_message(panic_payload.as_ref());
                    error!(
                        "Transcription engine panicked: {}. Model has been unloaded.",
                        panic_msg
                    );

                    // Clear the model ID so it will be reloaded on next attempt
                    {
                        let mut current_model = self
                            .current_model_id
                            .lock()
                            .unwrap_or_else(|e| e.into_inner());
                        *current_model = None;
                    }

                    let _ = self.app_handle.emit(
                        "model-state-changed",
                        ModelStateEvent {
                            event_type: "unloaded".to_string(),
                            model_id: None,
                            model_name: None,
                            error: Some(format!("Engine panicked: {}", panic_msg)),
                        },
                    );

                    return Err(SttError::Provider(format!(
                        "Transcription engine panicked: {}. \
                         The model has been unloaded and will reload on next attempt.",
                        panic_msg
                    )));
                }
            };

            let output_language = with_model_detected_language(
                resolve_output_language_evidence(
                    &settings,
                    applied_language_hint.as_deref(),
                    &model_languages,
                    output_was_translated,
                ),
                model_detected_language,
            );
            debug!("Output language evidence: {:?}", output_language);

            (text, output_language, model_languages)
        };
        let provider_latency_ms = engine_start.elapsed().as_millis() as u32;

        // Apply fuzzy word correction if custom words are configured — UNLESS the
        // words were already handed to the model as an initial prompt (whisper
        // family). We don't pass a prompt to non-whisper models (it requires the
        // whisper-kind run extension), so they still get fuzzy correction here,
        // same as the ONNX engines.
        let filtered_result = post_process_transcription_text(
            result,
            &settings,
            model_is_whisper,
            &output_language,
            &model_languages,
        );

        let et = std::time::Instant::now();
        let translation_note = if settings.translate_to_english {
            " (translated)"
        } else {
            ""
        };
        // Real-time factor. Input PCM is 16 kHz mono, so audio length in seconds
        // is samples / 16000. `speedup` is audio_secs / elapsed_secs — e.g. 4.00x
        // means transcribed 4x faster than real time
        let elapsed_secs = (et - st).as_secs_f64();
        let audio_secs = audio_len as f64 / 16_000.0;
        let speedup = real_time_factor(audio_secs, elapsed_secs);
        info!(
            "Transcription completed in {:.2}s for {:.2}s of audio ({:.2}x real-time){}",
            elapsed_secs, audio_secs, speedup, translation_note
        );

        let final_result = filtered_result;

        if final_result.is_empty() {
            info!("Transcription result is empty");
        } else {
            info!(
                "Transcription result: {}",
                crate::utils::redact_text(&final_result)
            );
        }

        self.maybe_unload_immediately("transcription");

        Ok(Transcript {
            text: final_result,
            language: output_language
                .language()
                .map(|language| language.to_string()),
            // A v1 não produz segmentos com timestamps (FR-003-15).
            segments: Vec::new(),
            provider_latency_ms,
        })
    }
}
