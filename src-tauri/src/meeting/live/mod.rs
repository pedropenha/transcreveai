//! Live meeting transcription (T-065; FR-009-10, FR-009-15, AC-009-03).
//!
//! One worker thread per meeting drains two bounded FIFO queues — one per
//! [`Track`] — fairly interleaved, so local model inference stays serialized
//! while neither track starves the other. Live chunks (~8 s in-memory
//! slices) and sealed 60 s blocks are subsegmented by VAD into utterances
//! of at most 30 s ([`segment::MAX_UTTERANCE_MS`]); each utterance goes
//! through the meeting STT provider (`SttOrchestrator` + `LocalSttProvider`,
//! which already performs the one retry) and lands as a `meeting_segments`
//! row plus a `meeting://segment` event.
//!
//! Chunk/block overlap is deduplicated by coverage: a chunk merges its
//! persisted span into its track's [`Coverage`] interval set (never past a
//! still-open trailing utterance), and the sealing block transcribes only
//! each utterance's uncovered portions — live text is never inserted
//! twice, and a dropped chunk leaves a hole the block re-covers rather
//! than a watermark skipping past it.
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
pub(crate) use process::{
    drop_open_tail, process_utterances, record_coverage, uncovered, BlockPass, Coverage, Dedup,
    ResumePoint,
};
pub(crate) use queue::{BlockJob, TrackQueues};
pub(crate) use segment::{fixed_chunks, segment_utterances, SegmenterConfig, Utterance};

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
pub(crate) enum LiveWork {
    Block(SealedBlock),
    Chunk {
        track: Track,
        start_offset_ms: u64,
        samples: Arc<Vec<f32>>,
    },
}

/// Backlog kept per track — 16 blocks ≈ 16 minutes of audio. Beyond that a
/// new block simply stays pending (`meeting_blocks.transcribed = 0`) for
/// T-067; nothing is dropped silently.
const QUEUE_CAPACITY_PER_TRACK: usize = 16;
/// Live chunks get a smaller slice of the track FIFO — they are best-effort
/// ahead of their sealing block, so a burst of stale chunks can never evict
/// a persisted block's live pass.
const CHUNK_QUEUE_BUDGET: usize = 4;
/// Bound on the work channel itself: an unbounded channel would let ~512 KB
/// chunks accumulate without limit while a slow job runs. Blocks survive a
/// full channel (the row stays pending); chunks are dropped — their block
/// re-covers them.
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
    tx: SyncSender<LiveWork>,
    /// A panicked/ended transcriber fails every send — warn once, not per
    /// chunk, and never silently.
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
    /// pending, so T-067 re-covers it.
    pub fn enqueue(&self, block: SealedBlock) {
        match self.tx.try_send(LiveWork::Block(block)) {
            Ok(()) => {}
            Err(mpsc::TrySendError::Full(_)) => {
                warn!("Meeting transcriber channel is full — the block stays pending for post-processing");
            }
            Err(mpsc::TrySendError::Disconnected(_)) => self.warn_closed_once(),
        }
    }

    /// Offer a live chunk — an in-memory slice ahead of the block seal.
    /// Chunks are best-effort: if the channel is full or the thread is
    /// gone, the sealing block re-covers the range, so nothing is lost.
    pub fn enqueue_chunk(&self, track: Track, start_offset_ms: u64, samples: Arc<Vec<f32>>) {
        match self.tx.try_send(LiveWork::Chunk {
            track,
            start_offset_ms,
            samples,
        }) {
            Ok(()) => {}
            Err(mpsc::TrySendError::Full(_)) => {
                debug!("Meeting transcriber channel full — dropping a live chunk; its block re-covers the range");
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
    rx: Receiver<LiveWork>,
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
        tm,
        dictation,
        queues: TrackQueues::new(QUEUE_CAPACITY_PER_TRACK),
        vad,
        segmenter: SegmenterConfig::default(),
        language,
        vocabulary,
        covered: [Vec::new(), Vec::new()],
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

fn slot(track: Track) -> usize {
    match track {
        Track::Mic => 0,
        Track::System => 1,
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
    /// `None` → fixed 30 s chunks (VAD init failed).
    vad: Option<Box<dyn VoiceActivityDetector>>,
    segmenter: SegmenterConfig,
    language: Option<String>,
    vocabulary: Vec<String>,
    /// Per-track meeting-clock ranges already persisted by live chunks —
    /// a *set* of intervals, not a flat watermark: a dropped chunk must
    /// leave a hole its sealing block re-covers, not silently swallow
    /// every utterance a flat watermark would skip past it.
    covered: [Coverage; 2],
    /// Consecutive blocks whose pass failed — drives the one-shot toast.
    failed_blocks: u32,
    warned: bool,
}

impl Runner {
    fn run(&mut self, rx: Receiver<LiveWork>) {
        loop {
            // Ingest everything already waiting so `pop` always sees the
            // freshest backlog of both tracks.
            while let Ok(work) = rx.try_recv() {
                self.offer(work);
            }
            if let Some(job) = self.queues.pop() {
                self.process(job);
                continue;
            }
            match rx.recv() {
                Ok(work) => self.offer(work),
                // Channel closed and queues drained — the meeting is over.
                Err(_) => break,
            }
        }
        debug!("Meeting transcriber exited for {}", self.meeting_id);
    }

    fn offer(&mut self, work: LiveWork) {
        match work {
            LiveWork::Block(block) => {
                let index = block.index;
                let track = block.track;
                let job = BlockJob::new(block);
                if let Err(job) = self.queues.push(job) {
                    // A burst of stale chunks must never evict a persisted
                    // block's live pass — drop the oldest chunk for headroom.
                    let made_room = self.queues.evict_oldest_chunk(track);
                    if !made_room || self.queues.push(job).is_err() {
                        warn!(
                            "Meeting {} {:?} block {} exceeds the live queue — stays pending for post-processing",
                            self.meeting_id, track, index
                        );
                    }
                }
            }
            LiveWork::Chunk {
                track,
                start_offset_ms,
                samples,
            } => {
                // Chunks share the FIFO but hold a smaller slice of it —
                // beyond the budget the stalest chunk makes room for the
                // freshest. A full FIFO still drops the chunk; either way
                // the sealing block re-covers the range, nothing is lost.
                while self.queues.chunks_queued(track) >= CHUNK_QUEUE_BUDGET {
                    if !self.queues.evict_oldest_chunk(track) {
                        break;
                    }
                }
                if self
                    .queues
                    .push(BlockJob::chunk(track, start_offset_ms, samples))
                    .is_err()
                {
                    debug!(
                        "Meeting {} {:?}: live chunk queue full — block pass re-covers the range",
                        self.meeting_id, track
                    );
                }
            }
        }
    }

    /// VAD-segment a decoded block (fresh detector state per block — the
    /// LSTM context must not leak across unrelated audio). Returns the
    /// utterances plus whether the trailing frame was voiced — a live chunk
    /// uses that to leave a still-open utterance for the block pass instead
    /// of persisting a clipped half.
    fn segment(&mut self, samples: &[f32]) -> (Vec<Utterance>, bool) {
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
                let tail_voiced = voiced.last().copied().unwrap_or(false);
                (
                    segment_utterances(samples.len(), frame, &voiced, &self.segmenter),
                    tail_voiced,
                )
            }
            None => (fixed_chunks(samples.len(), &self.segmenter), false),
        }
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
        let Some(_guard) = self.tm.try_start_loading() else {
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
            if job.bookkeep {
                warn!(
                    "Meeting {} {:?} block {}: model never became ready — stays pending",
                    self.meeting_id, job.block.track, job.block.index
                );
            } else {
                debug!(
                    "Meeting {} {:?}: live chunk gave up on a busy model — its block re-covers the range",
                    self.meeting_id, job.block.track
                );
            }
            self.finish_block(&job, false);
            return;
        }
        // A chunk whose whole range a previous pass already persisted
        // carries nothing new — drop it before paying for VAD + STT. Blocks
        // always run: they own `meeting_blocks` bookkeeping.
        if !job.bookkeep
            && uncovered(
                &self.covered[slot(job.block.track)],
                job.block.start_offset_ms as i64,
                job.block.start_offset_ms as i64 + job.block.duration_ms as i64,
            )
            .is_empty()
        {
            return;
        }
        let inline = job.inline.clone();
        let samples: Arc<Vec<f32>> = match inline {
            Some(samples) => samples,
            None => match read_wav_samples(&job.block.path) {
                Ok(samples) => Arc::new(samples),
                Err(e) => {
                    warn!(
                        "Meeting {} {:?} block {} unreadable ({e}) — stays pending",
                        self.meeting_id, job.block.track, job.block.index
                    );
                    self.finish_block(&job, false);
                    return;
                }
            },
        };
        let (mut utterances, tail_voiced) = self.segment(samples.as_slice());
        // A live chunk that ends mid-utterance must not persist the clipped
        // half: drop the open tail — the sealing block re-covers it. Only the
        // samples before it count as covered.
        let covered_samples = if job.bookkeep {
            samples.len()
        } else {
            drop_open_tail(&mut utterances, tail_voiced, samples.len())
        };
        // Utterances already persisted — by earlier live chunks, or by a
        // block that sealed before a straddling chunk ran — are skipped or
        // split out by the coverage set, so text is never inserted twice.
        let covered = &mut self.covered[slot(job.block.track)];
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
        let mut insert = |start_ms: i64, end_ms: i64, text: &str, excluded: bool| -> bool {
            let mut segment =
                MeetingSegment::new(meeting_id, track.label(), start_ms, end_ms, text);
            segment.excluded = excluded;
            if let Err(e) = SqliteMeetingSegmentRepository::new(conn).create(&segment) {
                // The row did not land — the portion must stay uncovered
                // (returning false fails the pass) or its text is lost.
                warn!("Failed to persist meeting segment: {e}");
                return false;
            }
            // The `meeting://segment` payload is the row itself
            // (contracts.md §5) — emitted only after the insert lands.
            if let Err(e) = app.emit(MEETING_SEGMENT_EVENT, &segment) {
                warn!("Failed to emit {MEETING_SEGMENT_EVENT}: {e}");
            }
            true
        };
        match process_utterances(
            &job.block,
            &utterances,
            Dedup {
                resume: ResumePoint {
                    utterance: job.resume_utterance,
                },
                covered,
            },
            samples.as_slice(),
            &mut transcribe,
            &mut insert,
            &is_excluded,
        ) {
            BlockPass::Done => {
                // Coverage records the full completed span: a chunk's
                // persisted range is [start, covered_samples) — the open
                // tail stays uncovered for the block — and a finished
                // block covers its whole range.
                let covered_end_ms = if job.bookkeep {
                    job.block.start_offset_ms + job.block.duration_ms
                } else {
                    job.block.start_offset_ms
                        + (covered_samples as u64 * 1_000) / WHISPER_SAMPLE_RATE as u64
                };
                record_coverage(
                    &mut self.covered[slot(job.block.track)],
                    job.block.start_offset_ms as i64,
                    covered_end_ms as i64,
                );
                self.finish_block(&job, true);
            }
            BlockPass::Failed => self.finish_block(&job, false),
            BlockPass::Busy(resume) => {
                if !job.bookkeep {
                    // A live chunk is best-effort: sleeping out the retry
                    // budget on this single thread would hold sealed blocks
                    // and fresher chunks hostage to a leased engine (a
                    // dictation in flight). Drop it — the sealing block
                    // re-covers the range.
                    debug!(
                        "Meeting {} {:?}: live chunk dropped on a busy model — its block re-covers the range",
                        self.meeting_id, job.block.track
                    );
                    self.finish_block(&job, false);
                    return;
                }
                job.resume_utterance = resume;
                job.busy_retries += 1;
                self.retry_model_load(&job);
                std::thread::sleep(BUSY_RETRY_DELAY);
                self.queues.requeue_front(job);
            }
        }
    }

    /// Flip the block's bookkeeping row and feed the failure-streak toast.
    /// Live chunks own no `meeting_blocks` row — a chunk failure only
    /// delays live text (the sealing block re-covers the range), so they
    /// skip both the db update and the warn streak.
    fn finish_block(&mut self, job: &BlockJob, ok: bool) {
        if job.bookkeep {
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
        }
        if ok {
            self.failed_blocks = 0;
            return;
        }
        if !job.bookkeep {
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
