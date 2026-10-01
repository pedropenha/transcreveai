//! FR-009-16 step 1 — transcribe pending sealed blocks.
//!
//! "Pending" is derived, never tracked: a block is pending when no `speech`
//! `meeting_segments` row overlaps its audio range (after `gap_marker`
//! subtraction). That single definition is what makes both the original pass
//! and `meeting_retry_processing` idempotent — T-064's worker already writes
//! the trailing-tail + gap-marker segments during shutdown, and a future
//! T-065 live transcriber's rows are absorbed by the same coverage test.
//!
//! Per-track/per-block failures degrade to a partial transcript (warn +
//! continue): the coverage bookkeeping means a retry only re-attempts what
//! is still uncovered. The step fails only when no STT model can be produced
//! at all — that is the retriable FR-009-22 case.

use std::path::Path;
use std::sync::Arc;

use tauri::Manager;

use crate::audio_toolkit::vad::{EarshotVad, SileroVad, VoiceActivityDetector};
use crate::commands::{CommandError, CommandErrorCode, CommandResult};
use crate::db::meeting_blocks::{
    MeetingBlock, MeetingBlockRepository, SqliteMeetingBlockRepository,
};
use crate::db::meetings::{
    Meeting, MeetingSegment, MeetingSegmentRepository, SqliteMeetingSegmentRepository,
};
use crate::managers::model::ModelManager;
use crate::managers::transcription::TranscriptionManager;
use crate::meeting::blocks::{scan_meeting_blocks, ScannedBlock, Track};
use crate::settings::{AppSettings, VadBackend};
use crate::stt::local::LocalSttProvider;
use crate::stt::orchestrator::SttOrchestrator;
use crate::stt::selection::effective_meeting_model_id;
use crate::stt::types::{AudioBuffer, SttOptions};

use super::coverage::{covered_ranges, pending_spans, resolve_spans, speech_regions, BlockSpan};
use super::prompt::speaker_for_track;
use super::{Worker, PCT_TRANSCRIBE_DONE, PCT_TRANSCRIBE_START, STEP_TRANSCRIBE};

/// FR-009-15: pending blocks are VAD-sliced into ≤ 30 s sub-segments.
const MAX_REGION_MS: u64 = 30_000;
/// Silence shorter than this keeps a speech region open — same constant the
/// recorder's offline profile uses (`VAD_OFFLINE_HANGOVER_MS`).
const MERGE_GAP_MS: u64 = crate::audio_toolkit::vad::VAD_OFFLINE_HANGOVER_MS;
/// VAD thresholds mirroring `managers::audio` (they are private there;
/// duplicated deliberately so post-processing does not depend on the
/// recorder's internals).
const SILERO_VAD_THRESHOLD: f32 = 0.3;
const EARSHOT_VAD_THRESHOLD: f32 = 0.5;

/// A sealed block that still needs transcription.
struct PendingBlock {
    track: Track,
    /// Owed lifetime — the caller keeps `scanned` alive for the whole step.
    block: ScannedBlock,
    span: BlockSpan,
}

impl Worker {
    /// Transcribe every pending sealed block for `meeting`.
    ///
    /// Errors:
    /// - `Internal` — audio dir/db unusable.
    /// - `Model` — no transcription model selected, or the meeting model
    ///   cannot be loaded. This is the *fatal* outcome the caller turns into
    ///   `status='error'` (FR-009-22).
    pub(crate) fn transcribe_pending(&mut self, meeting: &Meeting) -> CommandResult<()> {
        let Some(dir) = meeting.audio_dir.clone() else {
            return Ok(());
        };
        let scanned = scan_meeting_blocks(Path::new(&dir)).map_err(|e| {
            CommandError::logged(
                CommandErrorCode::Internal,
                "Failed to scan meeting audio",
                e,
            )
        })?;
        if scanned.blocks.is_empty() {
            return Ok(());
        }
        if !scanned.corrupt.is_empty() {
            log::warn!(
                "Meeting {}: {} corrupt audio block(s) skipped",
                meeting.id,
                scanned.corrupt.len()
            );
        }
        let segments = SqliteMeetingSegmentRepository::new(self.conn()?)
            .list_by_meeting(&meeting.id)
            .map_err(|e| {
                CommandError::logged(
                    CommandErrorCode::Internal,
                    "Failed to list meeting segments",
                    e,
                )
            })?;

        // Recorded clock placements of every sealed block (migration 11) —
        // authoritative when present; `resolve_spans` chains around gaps for
        // the rows a crash took (the fsync'd block outlives its row).
        let placements = SqliteMeetingBlockRepository::new(self.conn()?)
            .list_by_meeting(&meeting.id)
            .map_err(|e| {
                CommandError::logged(
                    CommandErrorCode::Internal,
                    "Failed to list meeting blocks",
                    e,
                )
            })?;
        let pending = Self::pending_blocks(&scanned.blocks, &segments, &placements);
        if pending.is_empty() {
            log::debug!("Meeting {}: all sealed blocks already covered", meeting.id);
            return Ok(());
        }

        // Resolve + load the meeting STT model (FR-003-03: inherits dictation
        // when unset). No model → the whole step fails (FR-009-22 retryable).
        let settings = crate::settings::get_settings(&self.app);
        let model_id = effective_meeting_model_id(&settings).ok_or_else(|| {
            CommandError::new(
                CommandErrorCode::Model,
                "No transcription model is selected for meetings",
            )
        })?;
        let tm = self
            .app
            .try_state::<Arc<TranscriptionManager>>()
            .map(|s| s.inner().clone())
            .ok_or_else(|| {
                CommandError::new(
                    CommandErrorCode::Internal,
                    "Transcription manager is unavailable",
                )
            })?;
        let mm = self
            .app
            .try_state::<Arc<ModelManager>>()
            .map(|s| s.inner().clone())
            .ok_or_else(|| {
                CommandError::new(CommandErrorCode::Internal, "Model manager is unavailable")
            })?;
        if !tm.is_model_loaded() || tm.get_current_model().as_deref() != Some(model_id.as_str()) {
            tm.load_model(&model_id).map_err(|e| {
                CommandError::logged(
                    CommandErrorCode::Model,
                    "Failed to load the meeting transcription model",
                    e,
                )
            })?;
        }
        let orchestrator = SttOrchestrator::new(
            Arc::new(LocalSttProvider::new(tm.as_ref().clone(), mm)),
            None,
        );
        let opts = SttOptions {
            // `meeting.language` was captured at capture start; "auto" stays
            // `None` so the engine auto-detects like dictation does.
            language: meeting
                .language
                .clone()
                .filter(|l| !l.is_empty() && l != "auto"),
            vocabulary_hints: settings.custom_words.clone(),
            diarize: false,
            timeout: SttOrchestrator::dictation_timeout(MAX_REGION_MS as f64 / 1000.0),
        };
        let mut vad = self.make_vad(&settings).ok_or_else(|| {
            CommandError::new(
                CommandErrorCode::Internal,
                "Failed to initialize the speech detector",
            )
        })?;

        let total = pending.len();
        for (done, pending) in pending.iter().enumerate() {
            match self.transcribe_block(&orchestrator, vad.as_mut(), &opts, meeting, pending) {
                Ok(produced) => {
                    log::debug!(
                        "Meeting {}: block {}-{:04} produced {} segment(s)",
                        meeting.id,
                        pending.track.label(),
                        pending.block.index,
                        produced
                    );
                    if let Ok(conn) = self.conn() {
                        let _ = SqliteMeetingBlockRepository::new(conn).mark_transcribed(
                            &meeting.id,
                            pending.track.label(),
                            pending.block.index as i64,
                        );
                    }
                }
                // Track/block-level failure: keep the partial transcript —
                // already-inserted segments stay and a retry only revisits
                // the still-uncovered blocks.
                Err(e) => {
                    log::warn!(
                        "Meeting {}: transcription of {}-{:04} failed: {}",
                        meeting.id,
                        pending.track.label(),
                        pending.block.index,
                        e
                    );
                    if let Ok(conn) = self.conn() {
                        let _ = SqliteMeetingBlockRepository::new(conn).record_failure(
                            &meeting.id,
                            pending.track.label(),
                            pending.block.index as i64,
                        );
                    }
                }
            }
            let pct = PCT_TRANSCRIBE_START
                + ((done + 1) as u32 * (PCT_TRANSCRIBE_DONE - PCT_TRANSCRIBE_START)) / total as u32;
            self.emit_progress(&meeting.id, STEP_TRANSCRIBE, 1, pct);
        }
        Ok(())
    }

    /// The sealed blocks whose audio range has no covering `speech` segment —
    /// see [`coverage::pending_spans`]. `gap_marker`s (pauses/system-detach)
    /// shift the reconstructed clock inside `resolve_spans` so silent pauses
    /// never hit the engine. Pause markers land on `mic` but apply to both
    /// tracks (the capture stops entirely); `system` detach markers apply to
    /// `system` only — the mic keeps recording through a device swap.
    fn pending_blocks(
        blocks: &[ScannedBlock],
        segments: &[MeetingSegment],
        placements: &[MeetingBlock],
    ) -> Vec<PendingBlock> {
        let mut pending = Vec::new();
        for track in Track::ALL {
            let track_blocks: Vec<ScannedBlock> = blocks
                .iter()
                .filter(|b| b.track == track)
                .cloned()
                .collect();
            let placed: Vec<(u32, i64, i64)> = placements
                .iter()
                .filter(|p| p.track == track.label())
                .map(|p| (p.block_index as u32, p.start_ms, p.end_ms))
                .collect();
            let gaps: Vec<(i64, i64)> = segments
                .iter()
                .filter(|s| s.kind == "gap_marker")
                .filter(|s| match track {
                    Track::Mic => s.track == Track::Mic.label(),
                    Track::System => true,
                })
                .map(|s| (s.start_ms, s.end_ms))
                .collect();
            // 16 kHz mono → `sample_count / 16` ms per block.
            let descriptors: Vec<(u32, i64)> = track_blocks
                .iter()
                .map(|b| (b.index, b.sample_count as i64 / 16))
                .collect();
            let spans = resolve_spans(&descriptors, &placed, &gaps);
            let covered = covered_ranges(segments, track.label());
            for (block, span) in track_blocks.into_iter().zip(spans) {
                if !pending_spans(std::slice::from_ref(&span), &covered).is_empty() {
                    pending.push(PendingBlock { track, block, span });
                }
            }
        }
        pending
    }

    /// VAD configured like the recorder's (settings `vad_backend`), falling
    /// back to the model-free Earshot backend when Silero's ONNX file cannot
    /// be resolved.
    fn make_vad(&self, settings: &AppSettings) -> Option<Box<dyn VoiceActivityDetector>> {
        match settings.vad_backend {
            VadBackend::Silero => {
                let path = self
                    .app
                    .path()
                    .resolve(
                        "resources/models/silero_vad_v4.onnx",
                        tauri::path::BaseDirectory::Resource,
                    )
                    .ok()?;
                match SileroVad::new(path, SILERO_VAD_THRESHOLD) {
                    Ok(vad) => Some(Box::new(vad)),
                    Err(e) => {
                        log::warn!(
                            "Silero VAD unavailable for meeting blocks ({e}); using Earshot"
                        );
                        EarshotVad::new(EARSHOT_VAD_THRESHOLD)
                            .ok()
                            .map(|v| Box::new(v) as _)
                    }
                }
            }
            VadBackend::Earshot => EarshotVad::new(EARSHOT_VAD_THRESHOLD)
                .ok()
                .map(|v| Box::new(v) as _),
        }
    }

    /// Transcribe one pending block: VAD → ≤ 30 s speech regions → one STT
    /// call per region → `meeting_segments` rows + `meeting://segment`
    /// events. Speaker labels use the track defaults — `Você`/`Outros` —
    /// until step 3's diarization lands (P1).
    fn transcribe_block(
        &mut self,
        orchestrator: &SttOrchestrator,
        vad: &mut dyn VoiceActivityDetector,
        opts: &SttOptions,
        meeting: &Meeting,
        pending: &PendingBlock,
    ) -> anyhow::Result<usize> {
        let samples = crate::audio_toolkit::read_wav_samples(&pending.block.path)?;
        let frame_samples = vad.frame_samples();
        let frame_ms = (frame_samples as u64 * 1000 / 16_000).max(1);
        vad.reset();

        // Per-frame verdicts; VAD errors fail open — a garbled frame is kept
        // as speech rather than silently dropping meeting audio.
        let mut voiced = Vec::with_capacity(samples.len() / frame_samples + 1);
        for chunk in samples.chunks(frame_samples) {
            let verdict = if chunk.len() == frame_samples {
                vad.is_voice(chunk).unwrap_or(true)
            } else {
                // Tail: pad to a full frame — VADs demand exact sizes.
                let mut padded = chunk.to_vec();
                padded.resize(frame_samples, 0.0);
                vad.is_voice(&padded).unwrap_or(true)
            };
            voiced.push(verdict);
        }

        let regions = speech_regions(&voiced, frame_ms, MERGE_GAP_MS, MAX_REGION_MS);
        let mut produced = 0usize;
        let speaker = speaker_for_track(pending.track);
        for (region_start_ms, region_end_ms) in regions {
            let sample_start = (region_start_ms * 16) as usize;
            let sample_end = ((region_end_ms * 16) as usize).min(samples.len());
            if sample_start >= sample_end {
                continue;
            }
            let audio = AudioBuffer::dictation(samples[sample_start..sample_end].to_vec());
            let outcome = orchestrator
                .transcribe_blocking(audio, opts)
                .map_err(|e| anyhow::anyhow!("meeting block transcription failed: {}", e))?;
            let text = outcome.transcript.text.trim().to_string();
            if text.is_empty() {
                continue;
            }
            let mut segment = MeetingSegment::new(
                &meeting.id,
                pending.track.label(),
                pending.span.start_ms + region_start_ms as i64,
                pending.span.start_ms + region_end_ms as i64,
                &text,
            );
            segment.speaker = Some(speaker.to_string());
            let conn = self
                .conn_opt()
                .ok_or_else(|| anyhow::anyhow!("meeting database is unavailable"))?;
            SqliteMeetingSegmentRepository::new(conn).create(&segment)?;
            produced += 1;
            self.emit_segment(&segment);
        }
        Ok(produced)
    }
}
