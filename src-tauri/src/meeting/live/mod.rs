//! Live meeting transcription (T-065; FR-009-10, FR-009-15, AC-009-03).
//!
//! One worker thread per meeting drains two bounded FIFO queues — one per
//! [`Track`] — fairly interleaved, so local model inference stays serialized
//! while neither track starves the other. Work arrives as sealed 60 s
//! blocks; each block is transcribed in **one** engine call through the
//! meeting STT provider (`SttOrchestrator` + `LocalSttProvider`, which
//! already performs the one retry) and lands as a `meeting_segments` row
//! plus a `meeting://segment` event.
//!
//! **One call per block, not per utterance.** The A/B harness
//! (`ab_variants` + `scripts/meeting-ab.ps1`) measured the old per-utterance
//! slicing on a real meeting: 15% of utterances produced no text at all on
//! Nemotron, Whisper and Parakeet hallucinated English filler at the cut
//! edges, and sentences the engine could reconstruct came back in pieces.
//! One call per block removed all of it and ran three times faster. See
//! [`spans`] for what the VAD is still used for (speech presence and outer
//! trim) and `docs/adr/0005-transcricao-de-reuniao-por-bloco.md` for the
//! decision and what it cost: live text now lands when the block seals
//! (~60 s), not within 10 s, and the transcript's rows are block-grained.
//!
//! Dictation coexistence (FR-009-10): `session://state` transitions open and
//! close intervals on the meeting clock inside a shared
//! [`dictation::DictationTracker`]. A `mic` block overlapping a dictation
//! interval is split *at the interval boundaries* so only the dictated
//! stretch is persisted with `excluded = 1`, and the session worker writes
//! one `dictation_marker` ("Ditado") per closed interval — the `system`
//! track is never touched (AC-009-03).
//!
//! Pending handoff to post-processing (deliberate seam): the session worker
//! records every sealed block in `meeting_blocks` with `transcribed = 0` and
//! the transcriber flips the flag once the block was fully processed. Blocks
//! still queued at stop, rejected by a full queue, or skipped after an
//! exhausted retry stay pending — `transcribed = 0` rows are exactly the
//! post-processing queue. Post-processing derives WAV paths from
//! `meetings.audio_dir` plus `blocks::block_filename(track, block_index)`;
//! when it re-processes a pending block it deletes `speech` rows inside the
//! block range first, since a block split by a dictation interval may have
//! already persisted earlier spans before failing.

#[cfg(test)]
mod ab_variants;
mod dictation;
mod queue;
/// Per-utterance VAD slicing — the pre-block-pass behaviour. Production no
/// longer slices (see [`spans`]); the segmenter survives as the baseline the
/// A/B harness reproduces, so a future model or span change can be measured
/// against what shipped before.
#[cfg(test)]
mod segment;
mod spans;

pub(crate) use dictation::{
    dictation_capturing, dictation_marker_text, DictationTracker, SharedDictationTracker,
};
pub(crate) use queue::{BlockJob, TrackQueues};
pub(crate) use spans::{block_spans, SpeechSpan};

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::sync::Arc;
use std::time::{Duration, Instant};

use log::{debug, warn};
use rusqlite::Connection;
use tauri::{AppHandle, Emitter, Manager};

use crate::audio_toolkit::constants::WHISPER_SAMPLE_RATE;
use crate::audio_toolkit::read_wav_samples;
use crate::audio_toolkit::vad::{EarshotVad, SileroVad, VoiceActivityDetector};
use crate::db::meeting_blocks::{MeetingBlockRepository, SqliteMeetingBlockRepository};
use crate::db::meetings::{
    MeetingSegment, MeetingSegmentRepository, SqliteMeetingSegmentRepository,
};
use crate::managers::audio::{EARSHOT_VAD_THRESHOLD, SILERO_VAD_THRESHOLD};
use crate::managers::model::ModelManager;
use crate::managers::transcription::TranscriptionManager;
use crate::meeting::blocks::{SealedBlock, Track};
use crate::meeting::session::{open_session_db, ToastPayload, TOAST_SHOW_EVENT};
use crate::pipeline::PipelineInput;
use crate::settings::{get_settings, VadBackend};
use crate::stt::local::LocalSttProvider;
use crate::stt::orchestrator::SttOrchestrator;
use crate::stt::selection::effective_meeting_model_id;
use crate::stt::types::{AudioBuffer, SttError, SttOptions};

/// `meeting://segment` — contracts.md §5: the payload is the persisted
/// `MeetingSegment` row itself. Emitted after every successful insert
/// (`speech` and `dictation_marker` alike) so the meeting window appends
/// live.
pub const MEETING_SEGMENT_EVENT: &str = "meeting://segment";

/// Work arriving on the transcriber channel: persisted blocks (which own
/// `meeting_blocks` bookkeeping) and live chunks (in-memory slices emitted
/// ahead of the seal so the transcript lands inside the FR-009-15 latency
/// budget instead of waiting a minute).
/// Backlog kept per track — 16 blocks ≈ 16 minutes of audio. Beyond that a
/// new block simply stays pending (`meeting_blocks.transcribed = 0`) for
/// post-processing; nothing is dropped silently.
const QUEUE_CAPACITY_PER_TRACK: usize = 16;
/// Bound on the work channel itself, so a slow job cannot let the capture
/// path accumulate work without limit. A block survives a full channel: its
/// row stays pending.
const WORK_CHANNEL_CAPACITY: usize = 2 * QUEUE_CAPACITY_PER_TRACK + 8;
/// `SttError::ModelNotReady` means the engine is leased elsewhere (a
/// dictation in flight) or still loading — transient, so the block requeues
/// instead of failing. ~8 s of tolerance at [`BUSY_RETRY_DELAY`] spacing
/// before it gives up and stays pending.
const MAX_BUSY_RETRIES: u32 = 16;
const BUSY_RETRY_DELAY: Duration = Duration::from_millis(500);
/// Consecutive failed blocks that warrant one user-facing toast.
const FAILURE_WARN_AT: u32 = 3;

fn live_transcription_warning(lang: &str) -> String {
    if lang == "pt-BR" {
        "A transcrição ao vivo encontrou falhas — os trechos pendentes serão processados ao final."
    } else {
        "Live transcription hit failures — pending stretches will be processed when the meeting ends."
    }
    .to_string()
}

/// Handle the session worker holds. Dropping it closes the channel; the
/// thread drains its remaining queues and exits.
pub(crate) struct LiveTranscriber {
    tx: SyncSender<SealedBlock>,
    /// A panicked/ended transcriber fails every send — warn once, not per
    /// block, and never silently.
    closed_warned: AtomicBool,
}

impl LiveTranscriber {
    /// Spawn the per-meeting transcriber thread. `None` when the STT managers
    /// are unreachable (e.g. headless context) — blocks stay pending either
    /// way, so live transcription can simply not exist.
    pub fn spawn(
        app: &AppHandle,
        meeting_id: &str,
        dictation: SharedDictationTracker,
    ) -> Option<Self> {
        let tm = app
            .try_state::<Arc<TranscriptionManager>>()?
            .inner()
            .clone();
        let mm = app.try_state::<Arc<ModelManager>>()?.inner().clone();
        let (tx, rx) = mpsc::sync_channel(WORK_CHANNEL_CAPACITY);
        let app = app.clone();
        let meeting_id = meeting_id.to_string();
        let spawned = std::thread::Builder::new()
            .name("meeting-transcriber".to_string())
            .spawn(move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    run_thread(app, meeting_id, dictation, tm, mm, rx);
                }));
                if let Err(e) = result {
                    log::error!("Meeting transcriber panicked: {e:?}");
                }
            });
        match spawned {
            Ok(_) => Some(Self {
                tx,
                closed_warned: AtomicBool::new(false),
            }),
            Err(e) => {
                log::error!("Failed to spawn meeting transcriber: {e}");
                None
            }
        }
    }

    fn warn_closed_once(&self) {
        if !self.closed_warned.swap(true, Ordering::Relaxed) {
            warn!("Meeting transcriber thread is gone — live transcription is off; sealed blocks stay pending for post-processing");
        }
    }

    /// Offer a sealed block — `try_send` never blocks the capture path. A
    /// full channel is fine: the block's `meeting_blocks` row is already
    /// pending, so post-processing picks it up.
    pub fn enqueue(&self, block: SealedBlock) {
        match self.tx.try_send(block) {
            Ok(()) => {}
            Err(mpsc::TrySendError::Full(_)) => {
                warn!("Meeting transcriber channel is full — the block stays pending for post-processing");
            }
            Err(mpsc::TrySendError::Disconnected(_)) => self.warn_closed_once(),
        }
    }
}

/// Thread body: resolve provider + VAD, then drain the queues until the
/// channel closes.
fn run_thread(
    app: AppHandle,
    meeting_id: String,
    dictation: SharedDictationTracker,
    tm: Arc<TranscriptionManager>,
    mm: Arc<ModelManager>,
    rx: Receiver<SealedBlock>,
) {
    let conn = match open_session_db(&app) {
        Ok(conn) => conn,
        Err(e) => {
            log::error!("Meeting transcriber cannot reach the db: {}", e.message);
            // Drain quietly so the channel never backs up; the blocks stay
            // pending for T-067.
            while rx.recv().is_ok() {}
            return;
        }
    };

    let settings = get_settings(&app);
    let language =
        (settings.selected_language != "auto").then(|| settings.selected_language.clone());
    let vocabulary = settings.custom_words.clone();
    let vad = build_detector(&app, settings.vad_backend);

    // FR-003-03: the meeting provider is a local model id in v1. Warm it up
    // when it differs from whatever is loaded — a load already in flight
    // (dictation, onboarding) is left alone: `transcribe_once` waits on the
    // `is_loading` condvar and `ModelNotReady` requeues cover the gap.
    if let Some(model_id) = effective_meeting_model_id(&settings) {
        let already = tm.get_current_model().as_deref() == Some(model_id.as_str());
        if !already {
            if let Some(_guard) = tm.try_start_loading_for(&model_id) {
                if let Err(e) = tm.load_model(&model_id) {
                    warn!("Meeting live transcription could not load model '{model_id}': {e}");
                }
            }
        }
    } else {
        warn!("Meeting {meeting_id}: no STT model configured — live transcription idles");
    }

    let mut runner = Runner {
        app,
        meeting_id,
        conn,
        orchestrator: SttOrchestrator::new(
            Arc::new(LocalSttProvider::new(tm.as_ref().clone(), mm)),
            None,
        ),
        tm,
        dictation,
        queues: TrackQueues::new(QUEUE_CAPACITY_PER_TRACK),
        vad,
        language,
        vocabulary,
        text_pipeline: transcript_pipeline(&settings),
        failed_blocks: 0,
        warned: false,
    };
    runner.run(rx);
}

/// The configured VAD backend for offline block segmentation — the same
/// Silero/Earshot choice as dictation capture (`settings.vad_backend`).
fn build_detector(app: &AppHandle, backend: VadBackend) -> Option<Box<dyn VoiceActivityDetector>> {
    match backend {
        VadBackend::Silero => {
            let path = app
                .path()
                .resolve(
                    "resources/models/silero_vad_v4.onnx",
                    tauri::path::BaseDirectory::Resource,
                )
                .ok()?;
            match SileroVad::new(path, SILERO_VAD_THRESHOLD) {
                Ok(vad) => Some(Box::new(vad)),
                Err(e) => {
                    warn!("Meeting transcription VAD unavailable: {e}");
                    None
                }
            }
        }
        VadBackend::Earshot => match EarshotVad::new(EARSHOT_VAD_THRESHOLD) {
            Ok(vad) => Some(Box::new(vad)),
            Err(e) => {
                warn!("Meeting transcription VAD unavailable: {e}");
                None
            }
        },
    }
}

/// The deterministic text pipeline applied to every meeting segment
/// (normalize → dictionary → `light` cleanup), built once from the settings
/// snapshot the meeting started with.
///
/// Dictation has always run this (`actions.rs`); meeting segments used to go
/// to the database raw, which is part of why the same engine looked worse in
/// a meeting than in dictation — no sentence capitalisation, no filler
/// removal, no dictionary correction. `run_transcript` deliberately skips
/// the voice-command stage: see its doc.
pub(crate) fn transcript_pipeline(settings: &crate::settings::AppSettings) -> PipelineInput {
    PipelineInput {
        text: String::new(),
        language: (settings.selected_language != "auto")
            .then(|| settings.selected_language.clone()),
        cleanup_level: settings.cleanup_level,
        cleanup_filler_words: settings.custom_filler_words.clone(),
        filler_removal_enabled: settings.filler_word_removal_enabled,
        custom_words: settings.custom_words.clone(),
        word_correction_threshold: settings.word_correction_threshold,
        // Unused by `run_transcript` — a meeting is not dictation.
        voice_command_phrases: Default::default(),
        voice_submit_phrases: Default::default(),
        spoken_punctuation_enabled: false,
    }
}

/// Run the text pipeline over one segment, fail-open: a panic in the
/// pipeline must never discard text the engine already produced.
pub(crate) fn transcript_text(template: &PipelineInput, raw: &str) -> String {
    let input = PipelineInput {
        text: raw.to_string(),
        ..template.clone()
    };
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        crate::pipeline::run_transcript(&input)
    })) {
        Ok(text) if !text.is_empty() => text,
        Ok(_) => raw.to_string(),
        Err(_) => {
            log::error!("Meeting text pipeline panicked — keeping the raw transcript");
            raw.to_string()
        }
    }
}

struct Runner {
    app: AppHandle,
    meeting_id: String,
    conn: Connection,
    orchestrator: SttOrchestrator,
    /// Model lifecycle handle — on a `Busy` pass an absent engine is
    /// reloaded here instead of waiting out the retry budget.
    tm: Arc<TranscriptionManager>,
    dictation: SharedDictationTracker,
    queues: TrackQueues,
    /// `None` → the whole block is transcribed untrimmed (VAD init failed).
    vad: Option<Box<dyn VoiceActivityDetector>>,
    language: Option<String>,
    vocabulary: Vec<String>,
    /// Template for the per-segment text pipeline (see
    /// [`transcript_pipeline`]).
    text_pipeline: PipelineInput,
    /// Consecutive blocks whose pass failed — drives the one-shot toast.
    failed_blocks: u32,
    warned: bool,
}

impl Runner {
    fn run(&mut self, rx: Receiver<SealedBlock>) {
        loop {
            // Ingest everything already waiting so `pop` always sees the
            // freshest backlog of both tracks.
            while let Ok(block) = rx.try_recv() {
                self.offer(block);
            }
            if let Some(job) = self.queues.pop() {
                self.process(job);
                continue;
            }
            match rx.recv() {
                Ok(block) => self.offer(block),
                // Channel closed and queues drained — the meeting is over.
                Err(_) => break,
            }
        }
        debug!("Meeting transcriber exited for {}", self.meeting_id);
    }

    fn offer(&mut self, block: SealedBlock) {
        let index = block.index;
        let track = block.track;
        if self.queues.push(BlockJob::new(block)).is_err() {
            warn!(
                "Meeting {} {:?} block {} exceeds the live queue — stays pending for post-processing",
                self.meeting_id, track, index
            );
        }
    }

    /// The spans of a decoded block that go to the engine.
    ///
    /// The VAD runs with fresh state per block (the LSTM context must not
    /// leak across unrelated audio) and only answers "is there speech, and
    /// where does it start and end" — [`spans::block_spans`] does the rest.
    /// Without a VAD the whole block is transcribed untrimmed: feeding the
    /// engine 60 s that may be silent is better than dropping real speech.
    fn spans_for(&mut self, block: &SealedBlock, samples: &[f32]) -> Vec<SpeechSpan> {
        let total_ms = (samples.len() as u64 * 1_000) / WHISPER_SAMPLE_RATE as u64;
        let dictation = self.dictation_spans(block, total_ms);
        let Some(vad) = self.vad.as_mut() else {
            return spans::split_whole_block(total_ms, &dictation);
        };
        vad.reset();
        let frame = vad.frame_samples();
        let mut voiced = Vec::with_capacity(samples.len().div_ceil(frame));
        for chunk in samples.chunks(frame) {
            if chunk.len() == frame {
                // Fail-open like the recorder: an errored frame counts as
                // voiced, never silently dropped.
                voiced.push(vad.is_voice(chunk).unwrap_or(true));
            } else {
                let mut padded = vec![0.0f32; frame];
                padded[..chunk.len()].copy_from_slice(chunk);
                voiced.push(vad.is_voice(&padded).unwrap_or(true));
            }
        }
        let frame_ms = (frame as u64 * 1_000 / WHISPER_SAMPLE_RATE as u64).max(1);
        block_spans(total_ms, frame_ms, &voiced, &dictation)
    }

    /// Dictation intervals overlapping this block, in block-local ms. Always
    /// empty for `system`: AC-009-03 never excludes the remote track.
    fn dictation_spans(&self, block: &SealedBlock, total_ms: u64) -> Vec<(u64, u64)> {
        if block.track != Track::Mic {
            return Vec::new();
        }
        let start = block.start_offset_ms as i64;
        let end = start + total_ms as i64;
        self.dictation
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .intervals_within(start, end)
            .into_iter()
            .map(|(a, b)| ((a - start).max(0) as u64, (b - start).max(0) as u64))
            .collect()
    }

    /// `Busy` means `ModelNotReady` — the engine is leased to a dictation
    /// (transient; wait it out) or simply absent. An absent engine never
    /// fixes itself: the spawn-time load may have failed, or the model was
    /// never used this session. Retry the load here, bounded by the job's
    /// `busy_retries` budget.
    fn retry_model_load(&mut self, job: &BlockJob) {
        if self.tm.is_model_loaded() {
            return;
        }
        let settings = get_settings(&self.app);
        let Some(model_id) = effective_meeting_model_id(&settings) else {
            warn!(
                "Meeting {}: no meeting model configured — a Busy pass cannot self-heal",
                self.meeting_id
            );
            return;
        };
        let Some(_guard) = self.tm.try_start_loading_for(&model_id) else {
            // A load is already in flight — `transcribe_once` waits on the
            // condvar, so the next retry sees the loaded engine.
            debug!(
                "Meeting {} {:?} block {}: model load already in flight — waiting",
                self.meeting_id, job.block.track, job.block.index
            );
            return;
        };
        if let Err(e) = self.tm.load_model(&model_id) {
            warn!(
                "Meeting {} {:?} block {}: model '{model_id}' reload failed ({e}) — stays queued",
                self.meeting_id, job.block.track, job.block.index
            );
        }
    }

    fn process(&mut self, mut job: BlockJob) {
        if job.busy_retries >= MAX_BUSY_RETRIES {
            warn!(
                "Meeting {} {:?} block {}: model never became ready — stays pending",
                self.meeting_id, job.block.track, job.block.index
            );
            self.finish_block(&job, false);
            return;
        }
        let samples = match read_wav_samples(&job.block.path) {
            Ok(samples) => samples,
            Err(e) => {
                warn!(
                    "Meeting {} {:?} block {} unreadable ({e}) — stays pending",
                    self.meeting_id, job.block.track, job.block.index
                );
                self.finish_block(&job, false);
                return;
            }
        };

        let spans = self.spans_for(&job.block, &samples);
        if spans.is_empty() {
            // "Nada ouvido" for a whole block (FR-002-14's floor applied per
            // block). Silence is a *successful* pass: marking it failed would
            // send the row back to post-processing to find the same silence.
            debug!(
                "Meeting {} {:?} block {}: no speech — nothing to transcribe",
                self.meeting_id, job.block.track, job.block.index
            );
            self.finish_block(&job, true);
            return;
        }

        // Usually one span; more only when a dictation interval split the
        // block. `resume_span` skips what an earlier busy requeue settled —
        // which is sound because the spans are deterministic: the detector
        // is reset per block and the samples come from the sealed file, so
        // a requeued block recomputes the same list and the same indices.
        // (A dictation interval that *closed* between passes can only make
        // the list differ; `end` never moves an interval's start, so a
        // settled prefix stays settled.)
        for index in job.resume_span..spans.len() {
            let span = spans[index];
            let (start, end) = span.samples(samples.len());
            if start >= end {
                continue;
            }
            match self.transcribe_span(&samples[start..end]) {
                Ok(text) => {
                    let text = text.trim();
                    if text.is_empty() {
                        continue;
                    }
                    if !self.insert_span(&job.block, &span, text) {
                        // The row did not land and its text would be lost —
                        // leave the block pending so the post-pass redoes it.
                        self.finish_block(&job, false);
                        return;
                    }
                }
                Err(SttError::ModelNotReady) => {
                    // The engine is leased to a dictation in flight, or
                    // absent. Keep the spans already persisted and come back
                    // to this one.
                    job.resume_span = index;
                    job.busy_retries += 1;
                    self.retry_model_load(&job);
                    std::thread::sleep(BUSY_RETRY_DELAY);
                    self.queues.requeue_front(job);
                    return;
                }
                Err(e) => {
                    // Errors never carry transcript text — logging is safe.
                    warn!(
                        "Meeting {} {:?} block {}: transcription failed: {e}",
                        self.meeting_id, job.block.track, job.block.index
                    );
                    self.finish_block(&job, false);
                    return;
                }
            }
        }
        self.finish_block(&job, true);
    }

    /// One engine call. The orchestrator performs its own single retry; a
    /// whole block gets `block_timeout`, not the dictation deadline.
    fn transcribe_span(&self, chunk: &[f32]) -> std::result::Result<String, SttError> {
        let audio = AudioBuffer {
            samples: Arc::from(chunk),
            sample_rate: WHISPER_SAMPLE_RATE,
            started_at: Instant::now(),
        };
        let opts = SttOptions {
            language: self.language.clone(),
            vocabulary_hints: self.vocabulary.clone(),
            diarize: false,
            timeout: SttOrchestrator::block_timeout(audio.duration_secs()),
        };
        self.orchestrator
            .transcribe_blocking(audio, &opts)
            .map(|outcome| outcome.transcript.text)
    }

    /// Persist one span and emit `meeting://segment`; `false` when the row
    /// did not land.
    fn insert_span(&self, block: &SealedBlock, span: &SpeechSpan, text: &str) -> bool {
        let start_ms = block.start_offset_ms as i64 + span.start_ms as i64;
        let end_ms = block.start_offset_ms as i64 + span.end_ms as i64;
        let text = transcript_text(&self.text_pipeline, text);
        let mut segment = MeetingSegment::new(
            &self.meeting_id,
            block.track.label(),
            start_ms,
            end_ms,
            &text,
        );
        segment.excluded = span.excluded;
        if let Err(e) = SqliteMeetingSegmentRepository::new(&self.conn).create(&segment) {
            warn!("Failed to persist meeting segment: {e}");
            return false;
        }
        // The `meeting://segment` payload is the row itself (contracts.md
        // §5) — emitted only after the insert lands.
        if let Err(e) = self.app.emit(MEETING_SEGMENT_EVENT, &segment) {
            warn!("Failed to emit {MEETING_SEGMENT_EVENT}: {e}");
        }
        true
    }

    /// Flip the block's bookkeeping row and feed the failure-streak toast.
    /// A failed block keeps `transcribed = 0`, which *is* the
    /// post-processing queue — nothing is lost, it is only late.
    fn finish_block(&mut self, job: &BlockJob, ok: bool) {
        let repo = SqliteMeetingBlockRepository::new(&self.conn);
        let result = if ok {
            repo.mark_transcribed(
                &self.meeting_id,
                job.block.track.label(),
                job.block.index as i64,
            )
        } else {
            repo.record_failure(
                &self.meeting_id,
                job.block.track.label(),
                job.block.index as i64,
            )
        };
        if let Err(e) = result {
            warn!("Failed to update meeting block bookkeeping: {e}");
        }
        if ok {
            self.failed_blocks = 0;
            return;
        }
        self.failed_blocks += 1;
        if self.failed_blocks >= FAILURE_WARN_AT && !self.warned {
            self.warned = true;
            let lang = get_settings(&self.app).app_language;
            if let Err(e) = self.app.emit(
                TOAST_SHOW_EVENT,
                ToastPayload {
                    kind: "meeting_warning".to_string(),
                    message: live_transcription_warning(&lang),
                    action: None,
                    meeting_id: None,
                },
            ) {
                warn!("Failed to emit {TOAST_SHOW_EVENT}: {e}");
            }
        }
    }
}
