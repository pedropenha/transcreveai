//! Live meeting transcription (T-065; FR-009-10, FR-009-15, AC-009-03).
//!
//! One worker thread per meeting drains two bounded FIFO queues — one per
//! [`Track`] — fairly interleaved, so local model inference stays serialized
//! while neither track starves the other. Sealed 60 s blocks are
//! subsegmented by VAD into utterances of at most 30 s
//! ([`segment::MAX_UTTERANCE_MS`]); each utterance goes through the meeting
//! STT provider (`SttOrchestrator` + `LocalSttProvider`, which already
//! performs the one retry) and lands as a `meeting_segments` row plus a
//! `meeting://segment` event.
//!
//! Dictation coexistence (FR-009-10): `session://state` transitions open and
//! close intervals on the meeting clock inside a shared
//! [`dictation::DictationTracker`]. `mic` utterances overlapping a dictation
//! interval are still transcribed but persisted with `excluded = 1`, and the
//! session worker writes one `dictation_marker` ("Ditado") per closed
//! interval — the `system` track is never touched (AC-009-03).
//!
//! Pending handoff to T-067 (deliberate seam): the session worker records
//! every sealed block in `meeting_blocks` with `transcribed = 0` and the
//! transcriber flips the flag once the block was fully processed. Blocks
//! still queued at stop, rejected by a full queue, or skipped after an
//! exhausted retry stay pending — `transcribed = 0` rows are exactly the
//! post-processing queue. T-067 derives WAV paths from `meetings.audio_dir`
//! plus `blocks::block_filename(track, block_index)`; when it re-processes a
//! pending block it should delete `speech` rows inside the block range
//! first, since a mid-block failure may have already persisted earlier
//! utterances.

mod dictation;
mod process;
mod queue;
mod segment;

pub(crate) use dictation::{
    dictation_capturing, dictation_marker_text, DictationTracker, SharedDictationTracker,
};
pub(crate) use process::{process_utterances, BlockPass};
pub(crate) use queue::{BlockJob, TrackQueues};
pub(crate) use segment::{fixed_chunks, segment_utterances, SegmenterConfig, Utterance};

use std::sync::mpsc::{self, Receiver, Sender};
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

/// Backlog kept per track — 16 blocks ≈ 16 minutes of audio. Beyond that a
/// new block simply stays pending (`meeting_blocks.transcribed = 0`) for
/// T-067; nothing is dropped silently.
const QUEUE_CAPACITY_PER_TRACK: usize = 16;
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
    tx: Sender<SealedBlock>,
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
        let (tx, rx) = mpsc::channel();
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
            Ok(_) => Some(Self { tx }),
            Err(e) => {
                log::error!("Failed to spawn meeting transcriber: {e}");
                None
            }
        }
    }

    /// Offer a sealed block — `mpsc` send never blocks the capture path.
    pub fn enqueue(&self, block: SealedBlock) {
        if self.tx.send(block).is_err() {
            debug!("Meeting transcriber channel closed before a block landed");
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
            if let Some(_guard) = tm.try_start_loading() {
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
        dictation,
        queues: TrackQueues::new(QUEUE_CAPACITY_PER_TRACK),
        vad,
        segmenter: SegmenterConfig::default(),
        language,
        vocabulary,
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

struct Runner {
    app: AppHandle,
    meeting_id: String,
    conn: Connection,
    orchestrator: SttOrchestrator,
    dictation: SharedDictationTracker,
    queues: TrackQueues,
    /// `None` → fixed 30 s chunks (VAD init failed).
    vad: Option<Box<dyn VoiceActivityDetector>>,
    segmenter: SegmenterConfig,
    language: Option<String>,
    vocabulary: Vec<String>,
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

    /// VAD-segment a decoded block (fresh detector state per block — the
    /// LSTM context must not leak across unrelated audio).
    fn segment(&mut self, samples: &[f32]) -> Vec<Utterance> {
        match self.vad.as_mut() {
            Some(vad) => {
                vad.reset();
                let frame = vad.frame_samples();
                let mut voiced = Vec::with_capacity(samples.len().div_ceil(frame));
                for chunk in samples.chunks(frame) {
                    if chunk.len() == frame {
                        // Fail-open like the recorder: an errored frame
                        // counts as voiced, never silently dropped.
                        voiced.push(vad.is_voice(chunk).unwrap_or(true));
                    } else {
                        let mut padded = vec![0.0f32; frame];
                        padded[..chunk.len()].copy_from_slice(chunk);
                        voiced.push(vad.is_voice(&padded).unwrap_or(true));
                    }
                }
                segment_utterances(samples.len(), frame, &voiced, &self.segmenter)
            }
            None => fixed_chunks(samples.len(), &self.segmenter),
        }
    }

    fn process(&mut self, mut job: BlockJob) {
        if job.busy_retries >= MAX_BUSY_RETRIES {
            warn!(
                "Meeting {} {:?} block {}: model never became ready — stays pending",
                self.meeting_id, job.block.track, job.block.index
            );
            self.finish_block(&job.block, false);
            return;
        }
        let samples = match read_wav_samples(&job.block.path) {
            Ok(samples) => samples,
            Err(e) => {
                warn!(
                    "Meeting {} {:?} block {} unreadable ({e}) — stays pending",
                    self.meeting_id, job.block.track, job.block.index
                );
                self.finish_block(&job.block, false);
                return;
            }
        };
        let utterances = self.segment(&samples);
        let is_mic = job.block.track == Track::Mic;
        let dictation = Arc::clone(&self.dictation);
        let is_excluded = move |start_ms: i64, end_ms: i64| -> bool {
            is_mic
                && dictation
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .overlaps(start_ms, end_ms)
        };
        let orchestrator = &self.orchestrator;
        let language = self.language.clone();
        let vocabulary = self.vocabulary.clone();
        let mut transcribe = |chunk: &[f32]| -> std::result::Result<String, SttError> {
            let audio = AudioBuffer {
                samples: Arc::from(chunk),
                sample_rate: WHISPER_SAMPLE_RATE,
                started_at: Instant::now(),
            };
            let opts = SttOptions {
                language: language.clone(),
                vocabulary_hints: vocabulary.clone(),
                diarize: false,
                timeout: SttOrchestrator::dictation_timeout(
                    chunk.len() as f64 / WHISPER_SAMPLE_RATE as f64,
                ),
            };
            orchestrator
                .transcribe_blocking(audio, &opts)
                .map(|outcome| outcome.transcript.text)
                .map_err(|e| {
                    // Errors never carry transcript text — logging is safe.
                    if !matches!(e, SttError::ModelNotReady) {
                        warn!("Meeting live transcription failed: {e}");
                    }
                    e
                })
        };
        let conn = &self.conn;
        let app = &self.app;
        let meeting_id = &self.meeting_id;
        let track = job.block.track;
        let mut insert = |start_ms: i64, end_ms: i64, text: &str, excluded: bool| {
            let mut segment =
                MeetingSegment::new(meeting_id, track.label(), start_ms, end_ms, text);
            segment.excluded = excluded;
            if let Err(e) = SqliteMeetingSegmentRepository::new(conn).create(&segment) {
                warn!("Failed to persist meeting segment: {e}");
                return;
            }
            // The `meeting://segment` payload is the row itself
            // (contracts.md §5) — emitted only after the insert lands.
            if let Err(e) = app.emit(MEETING_SEGMENT_EVENT, &segment) {
                warn!("Failed to emit {MEETING_SEGMENT_EVENT}: {e}");
            }
        };
        match process_utterances(
            &job.block,
            &utterances,
            job.resume_utterance,
            &samples,
            &mut transcribe,
            &mut insert,
            &is_excluded,
        ) {
            BlockPass::Done => self.finish_block(&job.block, true),
            BlockPass::Failed => self.finish_block(&job.block, false),
            BlockPass::Busy(resume) => {
                job.resume_utterance = resume;
                job.busy_retries += 1;
                std::thread::sleep(BUSY_RETRY_DELAY);
                self.queues.requeue_front(job);
            }
        }
    }

    /// Flip the block's bookkeeping row and feed the failure-streak toast.
    fn finish_block(&mut self, block: &SealedBlock, ok: bool) {
        let repo = SqliteMeetingBlockRepository::new(&self.conn);
        let result = if ok {
            repo.mark_transcribed(&self.meeting_id, block.track.label(), block.index as i64)
        } else {
            repo.record_failure(&self.meeting_id, block.track.label(), block.index as i64)
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
