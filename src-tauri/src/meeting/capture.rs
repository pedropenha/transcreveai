//! Meeting capture composition (FR-009-03..05): the `mic` track taps the one
//! shared microphone stream (`FrameTap::Raw` + `when_idle`, so frames keep
//! flowing between dictation sessions) and the `system` track runs the WASAPI
//! loopback; both feed [`BlockWriter`]s on dedicated writer threads so frame
//! callbacks never wait on disk.
//!
//! This is plumbing, not the session: T-064 owns start/pause/stop policy,
//! duration limits and consent; T-065 consumes the emitted block/lifecycle
//! events for live transcription and `gap_marker`s, plus the `LiveChunk`
//! feed that lands transcript text inside the FR-009-15 latency budget
//! without changing the 60 s persisted cadence.

use std::{
    collections::VecDeque,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc,
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};

use anyhow::Result;
use log::warn;

use crate::audio_toolkit::audio::loopback::{
    CpalLoopbackBackend, LoopbackBackend, SystemAudioCapture, SystemAudioEvent,
    SystemAudioEventCallback,
};
use crate::audio_toolkit::{constants::WHISPER_SAMPLE_RATE, AudioFrameCallback};

use super::blocks::{meeting_audio_dir, BlockWriter, SealedBlock, Track, BLOCK_DURATION};

/// Poll cadence for the loopback worker in production.
const LOOPBACK_POLL: Duration = Duration::from_millis(250);

/// What the session layer observes while a meeting captures.
#[derive(Clone, Debug)]
pub enum MeetingCaptureEvent {
    /// A 60 s block (or the stop-time tail) was written and fsync'd — safe
    /// points for live transcription (FR-009-15).
    BlockSealed(SealedBlock),
    /// A track could not start — e.g. the microphone held exclusively by
    /// another app (spec edge case: continue with the remaining track).
    TrackUnavailable { track: Track, message: String },
    /// A block failed to write/fsync (disk full, …). Already sealed blocks
    /// stay valid; the session decides whether to stop safely.
    WriteFailed { track: Track, message: String },
    /// System-track lifecycle: `Detached`..`Attached` spans are the audio the
    /// session marks with `gap_marker` (FR-009-04).
    System(SystemAudioEvent),
    /// FR-009-15 live feed: a short in-memory slice emitted well before the
    /// 60 s block seals, so live transcription does not wait for the
    /// persisted cadence. The block still owns persistence — a dropped or
    /// failed chunk is re-covered when its block is transcribed.
    LiveChunk {
        track: Track,
        /// Meeting-clock offset of the chunk's first sample.
        start_offset_ms: u64,
        samples: Arc<Vec<f32>>,
    },
}

/// Sealed blocks per track when the capture stops. Recovery never depends on
/// this in-memory list — the directory is the source of truth.
#[derive(Debug, Default)]
pub struct CaptureSummary {
    pub mic: Vec<SealedBlock>,
    pub system: Vec<SealedBlock>,
    /// Per-track delivery counters — the diagnostic answer to "was the
    /// system track actually capturing?" without decoding WAVs.
    pub mic_stats: TrackStats,
    pub system_stats: TrackStats,
}

/// What one writer thread saw from its source.
#[derive(Clone, Copy, Debug, Default)]
pub struct TrackStats {
    /// Frames the source callback delivered.
    pub frames: u64,
    /// Samples pushed into the block writer.
    pub samples: u64,
    /// Samples the live feed emitted as chunks (0 when live is off).
    pub live_samples: u64,
}

/// How the mic track attaches to the shared stream. Production passes
/// closures over `AudioRecordingManager::subscribe_frame_consumer` (which
/// uses `FrameTap::Raw` + `when_idle`); tests inject a fake feed.
pub struct MicTap {
    subscribe: Box<dyn Fn(AudioFrameCallback) -> std::result::Result<u64, String> + Send>,
    unsubscribe: Box<dyn Fn(u64) + Send>,
}

impl MicTap {
    pub fn new(
        subscribe: impl Fn(AudioFrameCallback) -> std::result::Result<u64, String> + Send + 'static,
        unsubscribe: impl Fn(u64) + Send + 'static,
    ) -> Self {
        Self {
            subscribe: Box::new(subscribe),
            unsubscribe: Box::new(unsubscribe),
        }
    }
}

/// Where the `system` track comes from (FR-009-01: "Chamada no computador"
/// captures it, "Presencial" does not).
pub enum SystemSource {
    /// Production: WASAPI loopback of the default render endpoint via cpal.
    Default,
    /// Injected backend — the test seam for `SystemAudioCapture`.
    Custom(Box<dyn LoopbackBackend>),
    /// In-person meetings: mic only.
    Disabled,
}

enum TrackMsg {
    Frame(Vec<f32>),
    Stop(mpsc::Sender<(Vec<SealedBlock>, TrackStats)>),
}

/// A writer thread owning one track's [`BlockWriter`]. Frame callbacks only
/// queue samples; sealing/fsync happen here off the audio path.
struct TrackPipeline {
    tx: mpsc::Sender<TrackMsg>,
    handle: JoinHandle<()>,
}

impl TrackPipeline {
    /// Drain the tail into a final block and join the thread. Blocks already
    /// sealed are returned even if the tail flush fails.
    fn shutdown(self) -> (Vec<SealedBlock>, TrackStats) {
        let (reply_tx, reply_rx) = mpsc::channel();
        let _ = self.tx.send(TrackMsg::Stop(reply_tx));
        let outcome = reply_rx.recv().unwrap_or_default();
        let _ = self.handle.join();
        outcome
    }
}

/// How a (re)started capture numbers and timestamps its blocks. On a fresh
/// meeting this is [`Self::default`]; on resume (FR-009-06) `time_base` stays
/// the meeting's start and `next_*_index` continue the on-disk numbering so
/// resumed blocks never overwrite earlier ones and their offsets keep the
/// pause gap.
#[derive(Clone)]
pub struct CaptureConfig {
    /// Samples per block (spec: 60 s at 16 kHz).
    pub block_samples: usize,
    /// Monotonic instant block offsets are measured from.
    pub time_base: Instant,
    /// 1-based next block index for the mic track.
    pub next_mic_index: u32,
    /// 1-based next block index for the system track.
    pub next_system_index: u32,
    /// FR-009-15: emit `LiveChunk` events every this many samples per track
    /// (0 disables them). Kept separate from `block_samples` so the 60 s
    /// persisted blocks and their recovery semantics do not change.
    pub live_chunk_samples: usize,
    /// Optional per-frame sink fed with every resampled 16 kHz system frame —
    /// the session uses it for the Flow Bar's meeting waveform: remote
    /// participants only ever reach the loopback track, so without metering
    /// it the pill reads flat while the user listens.
    pub system_level_cb: Option<AudioFrameCallback>,
}

/// Live-feed cadence: ~8 s balances sub-10 s transcript latency (FR-009-15)
/// against re-segmenting the VAD on every chunk.
pub const LIVE_CHUNK_DURATION: Duration = Duration::from_secs(8);

impl Default for CaptureConfig {
    fn default() -> Self {
        Self {
            block_samples: BLOCK_DURATION.as_secs() as usize * WHISPER_SAMPLE_RATE as usize,
            time_base: Instant::now(),
            next_mic_index: 1,
            next_system_index: 1,
            live_chunk_samples: 0,
            system_level_cb: None,
        }
    }
}

/// Spawn a writer thread for `track`. A `WriteFailed` event is emitted per
/// failed write; the writer keeps retrying on later pushes (the unsealed
/// samples are not dropped).
fn spawn_track_writer(
    dir: PathBuf,
    track: Track,
    block_samples: usize,
    t0: Instant,
    next_index: u32,
    live_chunk_samples: usize,
    events: mpsc::Sender<MeetingCaptureEvent>,
) -> Result<TrackPipeline> {
    let writer = BlockWriter::with_start_index(&dir, track, t0, block_samples, next_index)?;
    let (tx, rx) = mpsc::channel::<TrackMsg>();
    let handle = std::thread::Builder::new()
        .name(format!("meeting-{}-writer", track.label()))
        .spawn(move || run_track_writer(track, writer, live_chunk_samples, t0, &rx, &events))?;
    Ok(TrackPipeline { tx, handle })
}

/// `events.send` is `let _ =`-safe only if a dead receiver still gets a
/// visible trail — the first failure per writer warns so a stopped session
/// never silently swallows `BlockSealed`/`WriteFailed` bookkeeping.
fn emit_capture_event(
    events: &mpsc::Sender<MeetingCaptureEvent>,
    track: Track,
    warned: &mut bool,
    event: MeetingCaptureEvent,
) {
    if events.send(event).is_err() && !*warned {
        *warned = true;
        warn!("Meeting {track:?} writer lost the capture event channel — later lifecycle events are dropped");
    }
}

fn run_track_writer(
    track: Track,
    mut writer: BlockWriter,
    live_chunk_samples: usize,
    t0: Instant,
    rx: &mpsc::Receiver<TrackMsg>,
    events: &mpsc::Sender<MeetingCaptureEvent>,
) {
    let mut send_warned = false;
    // Live feed (FR-009-15): samples also accumulate here and leave as a
    // `LiveChunk` every `live_chunk_samples`. Each chunk's offset is on the
    // *block axis* — the enclosing block's start stamp plus the sample
    // position inside it — because that is the axis the block pass later
    // dedups against. Anchoring chunks at raw frame arrival instead drifts
    // them off the block axis whenever the writer stalls (a mid-seal burst
    // arrives "late") or the system track detaches mid-block (the gap
    // produces no samples, so the block compresses it while wall-clock
    // offsets would include it) — a shifted chunk range re-inserts the
    // same speech or leaves slivers the block re-transcribes forever.
    let mut live_buf: Vec<f32> = Vec::new();
    // Ledger parallel to `live_buf`: (block-axis offset ms, sample count)
    // per contiguous run inside one pending block. A frame that straddles
    // a seal splits into two entries — the next block re-anchors its start
    // to the boundary instant, so linear extrapolation past the seal would
    // sit on the wrong block's clock.
    let mut live_segs: VecDeque<(u64, usize)> = VecDeque::new();
    let mut stats = TrackStats::default();

    while let Ok(msg) = rx.recv() {
        match msg {
            TrackMsg::Frame(frame) => {
                let now = Instant::now();
                let sealed_before = writer.sealed().len();
                stats.frames += 1;
                stats.samples += frame.len() as u64;
                // The frame's ledger segments are computed BEFORE the push.
                // A failed push keeps the samples in `writer.pending` (they
                // seal on a later push), but they never reach the live feed
                // — so no chunk may claim a range the block only writes
                // later. The block pass re-covers the hole.
                let mut frame_segs: Vec<(u64, usize)> = Vec::new();
                if live_chunk_samples > 0 && !frame.is_empty() {
                    let now_ms = now.saturating_duration_since(t0).as_millis() as u64;
                    // `cap.max(1)`: a zero block_samples would make `push` a
                    // no-op anyway — the guard just keeps `%`/`cap - in`
                    // from dividing by zero.
                    let cap = writer.block_capacity().max(1);
                    // `pos` is the frame sample's position in the combined
                    // (pending + frame) buffer `push` seals. Block 0 keeps
                    // the recorded pending start; every later block in this
                    // push re-stamps its start `now` (seal's boundary), which
                    // is also what happens when a failed write left pending
                    // at/above capacity — no underflow, no stale anchor.
                    let mut pos = writer.pending_len();
                    let mut taken = 0;
                    while taken < frame.len() {
                        let block_start_ms = if pos < cap {
                            match writer.pending_start_offset_ms() {
                                Some(start) if pos > 0 => start,
                                _ => now_ms,
                            }
                        } else {
                            now_ms
                        };
                        let in_block = pos % cap;
                        let axis =
                            block_start_ms + (in_block as u64 * 1_000) / WHISPER_SAMPLE_RATE as u64;
                        let take = (frame.len() - taken).min(cap - in_block);
                        frame_segs.push((axis, take));
                        pos += take;
                        taken += take;
                    }
                }
                let push_result = writer.push(&frame, now);
                // Blocks this push sealed are persisted even when the push
                // failed partway — emit them so their `meeting_blocks` rows
                // (T-067's recovery input) exist before reporting the
                // failure.
                let sealed = writer.sealed()[sealed_before..].to_vec();
                for block in sealed {
                    emit_capture_event(
                        events,
                        track,
                        &mut send_warned,
                        MeetingCaptureEvent::BlockSealed(block),
                    );
                }
                if let Err(err) = push_result {
                    emit_capture_event(
                        events,
                        track,
                        &mut send_warned,
                        MeetingCaptureEvent::WriteFailed {
                            track,
                            message: err.to_string(),
                        },
                    );
                    continue;
                }
                if live_chunk_samples > 0 && !frame.is_empty() {
                    live_segs.extend(frame_segs);
                    live_buf.extend_from_slice(&frame);
                    while live_buf.len() >= live_chunk_samples {
                        let Some(&(start_offset_ms, _)) = live_segs.front() else {
                            // Invariant: the ledger covers every buffered
                            // sample. A chunk emitted without an anchor
                            // would corrupt the coverage set — drop the
                            // buffer rather than emit offset 0.
                            warn!(
                                "Meeting {track:?} live ledger lost its anchor — dropping pending chunk samples"
                            );
                            live_buf.clear();
                            break;
                        };
                        let chunk: Vec<f32> = live_buf.drain(..live_chunk_samples).collect();
                        let mut consumed = live_chunk_samples;
                        while consumed > 0 {
                            let Some((axis, count)) = live_segs.front_mut() else {
                                break;
                            };
                            let take = (*count).min(consumed);
                            *count -= take;
                            consumed -= take;
                            // The remainder of a partially-consumed segment
                            // starts `take` samples further into its block —
                            // advance its anchor so the next chunk opens at
                            // its own first sample, not the segment's.
                            *axis += (take as u64 * 1_000) / WHISPER_SAMPLE_RATE as u64;
                            if *count == 0 {
                                live_segs.pop_front();
                            }
                        }
                        stats.live_samples += chunk.len() as u64;
                        emit_capture_event(
                            events,
                            track,
                            &mut send_warned,
                            MeetingCaptureEvent::LiveChunk {
                                track,
                                start_offset_ms,
                                samples: Arc::new(chunk),
                            },
                        );
                    }
                }
            }
            TrackMsg::Stop(reply) => {
                let sealed_before = writer.sealed().len();
                if let Err(err) = writer.drain_tail(Instant::now()) {
                    emit_capture_event(
                        events,
                        track,
                        &mut send_warned,
                        MeetingCaptureEvent::WriteFailed {
                            track,
                            message: err.to_string(),
                        },
                    );
                }
                let sealed = writer.sealed()[sealed_before..].to_vec();
                for block in sealed {
                    emit_capture_event(
                        events,
                        track,
                        &mut send_warned,
                        MeetingCaptureEvent::BlockSealed(block),
                    );
                }
                let _ = reply.send((writer.into_sealed(), stats));
                return;
            }
        }
    }
    // Sender dropped without Stop: still flush the tail rather than lose it.
    let sealed_before = writer.sealed().len();
    if let Err(err) = writer.drain_tail(Instant::now()) {
        emit_capture_event(
            events,
            track,
            &mut send_warned,
            MeetingCaptureEvent::WriteFailed {
                track,
                message: err.to_string(),
            },
        );
    }
    let sealed = writer.sealed()[sealed_before..].to_vec();
    for block in sealed {
        emit_capture_event(
            events,
            track,
            &mut send_warned,
            MeetingCaptureEvent::BlockSealed(block),
        );
    }
}

/// One active meeting capture: two tracks of rotating WAV blocks plus the
/// loopback lifecycle events.
pub struct MeetingCapture {
    audio_dir: PathBuf,
    mic_writer: Option<TrackPipeline>,
    mic_subscriber_id: Option<u64>,
    mic_unsubscribe: Option<Box<dyn Fn(u64) + Send>>,
    system_writer: Option<TrackPipeline>,
    system_capture: Option<SystemAudioCapture>,
}

impl MeetingCapture {
    /// Directory this meeting's blocks go to (`audio/meetings/<id>`).
    pub fn audio_dir_for(app_data_dir: &std::path::Path, meeting_id: &str) -> PathBuf {
        meeting_audio_dir(app_data_dir, meeting_id)
    }

    /// Start capturing with the spec's 60 s blocks.
    pub fn start(
        audio_dir: PathBuf,
        mic: Option<MicTap>,
        system: SystemSource,
        events: mpsc::Sender<MeetingCaptureEvent>,
    ) -> Result<Self> {
        Self::start_with_config(audio_dir, mic, system, events, CaptureConfig::default())
    }

    /// Same as [`Self::start`] with an explicit block size — tests rotate
    /// blocks without waiting a minute.
    pub fn start_with_block_samples(
        audio_dir: PathBuf,
        mic: Option<MicTap>,
        system: SystemSource,
        events: mpsc::Sender<MeetingCaptureEvent>,
        block_samples: usize,
    ) -> Result<Self> {
        Self::start_with_config(
            audio_dir,
            mic,
            system,
            events,
            CaptureConfig {
                block_samples,
                ..CaptureConfig::default()
            },
        )
    }

    /// Full start: `config` carries the block size, the shared monotonic base
    /// and the resume indices (FR-009-06 pause/resume).
    pub fn start_with_config(
        audio_dir: PathBuf,
        mic: Option<MicTap>,
        system: SystemSource,
        events: mpsc::Sender<MeetingCaptureEvent>,
        config: CaptureConfig,
    ) -> Result<Self> {
        std::fs::create_dir_all(&audio_dir)?;
        // Shared monotonic base: `start_offset_ms` of mic and system blocks
        // are comparable across tracks (FR-009-03).
        let t0 = config.time_base;
        let block_samples = config.block_samples;

        let (mic_writer, mic_subscriber_id, mic_unsubscribe) = match mic {
            Some(tap) => {
                match spawn_track_writer(
                    audio_dir.clone(),
                    Track::Mic,
                    block_samples,
                    t0,
                    config.next_mic_index,
                    config.live_chunk_samples,
                    events.clone(),
                ) {
                    Ok(pipeline) => {
                        let tx = pipeline.tx.clone();
                        let send_warned = Arc::new(AtomicBool::new(false));
                        let frame_cb: AudioFrameCallback = Arc::new(move |frame| {
                            // A dead writer at ~100 Hz must warn once, not
                            // per frame — and never silently.
                            if tx.send(TrackMsg::Frame(frame.to_vec())).is_err()
                                && !send_warned.swap(true, Ordering::Relaxed)
                            {
                                warn!("Meeting mic writer thread is gone — frames are dropped");
                            }
                        });
                        match (tap.subscribe)(frame_cb) {
                            Ok(id) => (Some(pipeline), Some(id), Some(tap.unsubscribe)),
                            Err(message) => {
                                // Spec edge case: mic exclusively held by
                                // another app — warn and keep the system track.
                                warn!("Meeting mic track unavailable: {message}");
                                let _ = events.send(MeetingCaptureEvent::TrackUnavailable {
                                    track: Track::Mic,
                                    message,
                                });
                                pipeline.shutdown();
                                (None, None, None)
                            }
                        }
                    }
                    Err(err) => {
                        let _ = events.send(MeetingCaptureEvent::TrackUnavailable {
                            track: Track::Mic,
                            message: err.to_string(),
                        });
                        (None, None, None)
                    }
                }
            }
            None => (None, None, None),
        };

        let (system_writer, system_capture) = match system {
            SystemSource::Disabled => (None, None),
            source => {
                match spawn_track_writer(
                    audio_dir.clone(),
                    Track::System,
                    block_samples,
                    t0,
                    config.next_system_index,
                    config.live_chunk_samples,
                    events.clone(),
                ) {
                    Ok(pipeline) => {
                        let tx = pipeline.tx.clone();
                        let send_warned = Arc::new(AtomicBool::new(false));
                        // Level metering rides the same callback so the Flow
                        // Bar sees remote voice; it is fire-and-forget, never
                        // on the write path's critical section.
                        let level_cb = config.system_level_cb.clone();
                        let frame_cb: AudioFrameCallback = Arc::new(move |frame| {
                            if let Some(cb) = &level_cb {
                                cb(frame);
                            }
                            if tx.send(TrackMsg::Frame(frame.to_vec())).is_err()
                                && !send_warned.swap(true, Ordering::Relaxed)
                            {
                                warn!("Meeting system writer thread is gone — frames are dropped");
                            }
                        });
                        let event_tx = events.clone();
                        let event_cb: SystemAudioEventCallback = Arc::new(move |event| {
                            let _ = event_tx.send(MeetingCaptureEvent::System(event));
                        });
                        let backend: Box<dyn LoopbackBackend> = match source {
                            SystemSource::Default => Box::new(CpalLoopbackBackend),
                            SystemSource::Custom(backend) => backend,
                            SystemSource::Disabled => unreachable!("matched above"),
                        };
                        match SystemAudioCapture::start_with_backend(
                            backend,
                            LOOPBACK_POLL,
                            frame_cb,
                            event_cb,
                        ) {
                            Ok(capture) => (Some(pipeline), Some(capture)),
                            Err(message) => {
                                warn!("Meeting system track unavailable: {message}");
                                let _ = events.send(MeetingCaptureEvent::TrackUnavailable {
                                    track: Track::System,
                                    message,
                                });
                                pipeline.shutdown();
                                (None, None)
                            }
                        }
                    }
                    Err(err) => {
                        let _ = events.send(MeetingCaptureEvent::TrackUnavailable {
                            track: Track::System,
                            message: err.to_string(),
                        });
                        (None, None)
                    }
                }
            }
        };

        Ok(Self {
            audio_dir,
            mic_writer,
            mic_subscriber_id,
            mic_unsubscribe,
            system_writer,
            system_capture,
        })
    }

    /// Stop both tracks: detach the mic subscription first so no frames queue
    /// behind `Stop`, then stop the loopback and flush each writer's tail.
    pub fn stop(mut self) -> CaptureSummary {
        if let (Some(id), Some(unsubscribe)) =
            (self.mic_subscriber_id.take(), self.mic_unsubscribe.take())
        {
            unsubscribe(id);
        }
        if let Some(capture) = self.system_capture.take() {
            capture.stop();
        }
        let (mic, mic_stats) = self
            .mic_writer
            .take()
            .map(TrackPipeline::shutdown)
            .unwrap_or_default();
        let (system, system_stats) = self
            .system_writer
            .take()
            .map(TrackPipeline::shutdown)
            .unwrap_or_default();
        CaptureSummary {
            mic,
            system,
            mic_stats,
            system_stats,
        }
    }

    /// Directory holding this capture's blocks.
    pub fn audio_dir(&self) -> &std::path::Path {
        &self.audio_dir
    }

    /// Whether at least one track is capturing. When both tracks are
    /// unavailable (mic held exclusively AND loopback failed — e.g. an
    /// in-person meeting with no usable microphone) the meeting records
    /// nothing and the session layer refuses the start instead.
    pub fn is_capturing(&self) -> bool {
        self.mic_writer.is_some() || self.system_writer.is_some()
    }
}

impl Drop for MeetingCapture {
    fn drop(&mut self) {
        if let (Some(id), Some(unsubscribe)) =
            (self.mic_subscriber_id.take(), self.mic_unsubscribe.take())
        {
            unsubscribe(id);
        }
        if let Some(capture) = self.system_capture.take() {
            capture.stop();
        }
        // Dropping the writer senders ends the threads after they flush their
        // tails (see run_track_writer's disconnected path).
        for writer in [self.mic_writer.take(), self.system_writer.take()]
            .into_iter()
            .flatten()
        {
            writer.shutdown();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio_toolkit::audio::loopback::{DetachReason, LoopbackStream, OpenedLoopback};
    use rtrb::{Producer, RingBuffer};
    use std::sync::Mutex;

    /// Minimal fake loopback: always opens a 16 kHz stream whose ring the
    /// test can push samples into.
    struct FakeBackend {
        producer: Arc<Mutex<Option<Producer<f32>>>>,
    }

    struct FakeStream;

    impl LoopbackStream for FakeStream {
        fn device_name(&self) -> &str {
            "Fake Speakers"
        }
        fn sample_rate(&self) -> u32 {
            16_000
        }
        fn failure(&self) -> Option<String> {
            None
        }
    }

    impl LoopbackBackend for FakeBackend {
        fn default_device_name(&self) -> Result<Option<String>, String> {
            Ok(Some("Fake Speakers".to_string()))
        }
        fn open(&self) -> Result<OpenedLoopback, String> {
            let (producer, samples) = RingBuffer::new(32_000);
            *self.producer.lock().unwrap() = Some(producer);
            Ok(OpenedLoopback {
                stream: Box::new(FakeStream),
                samples,
            })
        }
    }

    fn feed(producer: &mut Producer<f32>, samples: &[f32]) {
        let chunk = producer
            .write_chunk_uninit(samples.len())
            .expect("test ring fits");
        chunk.fill_from_iter(samples.iter().copied());
    }

    fn wait_for(predicate: impl Fn() -> bool) -> bool {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if predicate() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        false
    }

    #[test]
    fn mic_and_system_frames_become_blocks() {
        let dir = tempfile::tempdir().unwrap();
        let (event_tx, event_rx) = mpsc::channel();

        // Fake mic feed: capture the callback so the test injects frames.
        let mic_cb: Arc<Mutex<Option<AudioFrameCallback>>> = Arc::new(Mutex::new(None));
        let unsubscribed = Arc::new(Mutex::new(None::<u64>));
        let tap = {
            let mic_cb = Arc::clone(&mic_cb);
            let unsubscribed = Arc::clone(&unsubscribed);
            MicTap::new(
                move |cb| {
                    *mic_cb.lock().unwrap() = Some(cb);
                    Ok(7)
                },
                move |id| *unsubscribed.lock().unwrap() = Some(id),
            )
        };

        let producer = Arc::new(Mutex::new(None::<Producer<f32>>));
        let backend = FakeBackend {
            producer: Arc::clone(&producer),
        };

        let capture = MeetingCapture::start_with_block_samples(
            dir.path().to_path_buf(),
            Some(tap),
            SystemSource::Custom(Box::new(backend)),
            event_tx,
            1_600, // 100 ms blocks at 16 kHz
        )
        .expect("capture starts");

        // Mic frames through the subscription callback.
        let cb = Arc::clone(&mic_cb);
        assert!(wait_for(|| cb.lock().unwrap().is_some()));
        let cb = cb.lock().unwrap().clone().unwrap();
        for _ in 0..3 {
            cb(&[0.5f32; 480]);
        }

        // System frames through the fake stream's ring.
        assert!(wait_for(|| producer.lock().unwrap().is_some()));
        {
            let mut guard = producer.lock().unwrap();
            let p = guard.as_mut().unwrap();
            for _ in 0..40 {
                feed(p, &[0.25f32; 80]); // 3200 samples total = 2 blocks
            }
        }

        assert!(wait_for(|| {
            event_rx.try_iter().any(|e| {
                matches!(
                    e,
                    MeetingCaptureEvent::System(SystemAudioEvent::Attached { .. })
                )
            })
        } || dir
            .path()
            .join("system-0001.wav")
            .exists()));

        let summary = capture.stop();
        // Mic pushed 1 440 samples < one block: only the stop-time tail seals.
        assert_eq!(summary.mic.len(), 1);
        assert_eq!(summary.mic[0].samples, 1_440);
        // System pushed 3 200 samples ≥ 2 full 100 ms blocks.
        assert!(
            summary.system.len() >= 2,
            "system blocks: {:?}",
            summary.system
        );

        let summary_dir = dir.path().to_path_buf();
        let found = super::super::blocks::scan_meeting_blocks(&summary_dir).unwrap();
        assert!(found
            .blocks
            .iter()
            .any(|b| b.track == Track::Mic && b.sample_count > 0));
        let system_samples: usize = found
            .blocks
            .iter()
            .filter(|b| b.track == Track::System)
            .map(|b| b.sample_count)
            .sum();
        assert!(
            system_samples >= 3_200,
            "system track kept every fed sample (blocks: {:?})",
            found
                .blocks
                .iter()
                .filter(|b| b.track == Track::System)
                .map(|b| (b.index, b.sample_count))
                .collect::<Vec<_>>()
        );
        assert_eq!(*unsubscribed.lock().unwrap(), Some(7));
    }

    #[test]
    fn system_frames_reach_the_level_sink() {
        // The Flow Bar's meeting waveform meters remote voice through
        // `system_level_cb` — without it the pill reads flat while the user
        // listens to the call. The sink sees every resampled frame the
        // writer gets, off the write path.
        use std::sync::atomic::AtomicU64;

        let dir = tempfile::tempdir().unwrap();
        let (event_tx, _event_rx) = mpsc::channel();
        let producer = Arc::new(Mutex::new(None::<Producer<f32>>));
        let backend = FakeBackend {
            producer: Arc::clone(&producer),
        };
        let metered = Arc::new(AtomicU64::new(0));
        let metered_cb = Arc::clone(&metered);
        let config = CaptureConfig {
            block_samples: 60_000, // nothing seals — irrelevant here
            system_level_cb: Some(Arc::new(move |frame: &[f32]| {
                metered_cb.fetch_add(frame.len() as u64, Ordering::Relaxed);
            })),
            ..CaptureConfig::default()
        };
        let capture = MeetingCapture::start_with_config(
            dir.path().to_path_buf(),
            None,
            SystemSource::Custom(Box::new(backend)),
            event_tx,
            config,
        )
        .expect("capture starts");

        assert!(wait_for(|| producer.lock().unwrap().is_some()));
        {
            let mut guard = producer.lock().unwrap();
            let p = guard.as_mut().unwrap();
            for _ in 0..40 {
                feed(p, &[0.25f32; 160]);
            }
        }
        // The resampler emits fixed-size frames, so the sink sees slightly
        // fewer than the fed total — substantial delivery proves the wiring.
        assert!(
            wait_for(|| metered.load(Ordering::Relaxed) >= 1_600),
            "the level sink saw only {} of the fed samples",
            metered.load(Ordering::Relaxed)
        );
        capture.stop();
    }

    #[test]
    fn mic_subscribe_failure_continues_system_only() {
        let dir = tempfile::tempdir().unwrap();
        let (event_tx, event_rx) = mpsc::channel();
        let tap = MicTap::new(|_| Err("mic busy".to_string()), |_| {});
        let backend = FakeBackend {
            producer: Arc::new(Mutex::new(None)),
        };

        let capture = MeetingCapture::start_with_block_samples(
            dir.path().to_path_buf(),
            Some(tap),
            SystemSource::Custom(Box::new(backend)),
            event_tx,
            1_600,
        )
        .expect("capture starts");

        let events: Vec<_> = event_rx.try_iter().collect();
        assert!(events.iter().any(|e| matches!(
            e,
            MeetingCaptureEvent::TrackUnavailable {
                track: Track::Mic,
                ..
            }
        )));

        capture.stop();
    }

    #[test]
    fn system_detach_and_reattach_surface_as_events() {
        // A scripted backend that swaps its default device name.
        struct SwapBackend {
            name: Arc<Mutex<String>>,
            producer: Arc<Mutex<Vec<Producer<f32>>>>,
        }
        impl LoopbackBackend for SwapBackend {
            fn default_device_name(&self) -> Result<Option<String>, String> {
                Ok(Some(self.name.lock().unwrap().clone()))
            }
            fn open(&self) -> Result<OpenedLoopback, String> {
                let name = self.name.lock().unwrap().clone();
                struct S(String);
                impl LoopbackStream for S {
                    fn device_name(&self) -> &str {
                        &self.0
                    }
                    fn sample_rate(&self) -> u32 {
                        16_000
                    }
                    fn failure(&self) -> Option<String> {
                        None
                    }
                }
                let (producer, samples) = RingBuffer::new(1_000);
                self.producer.lock().unwrap().push(producer);
                Ok(OpenedLoopback {
                    stream: Box::new(S(name)),
                    samples,
                })
            }
        }

        let dir = tempfile::tempdir().unwrap();
        let (event_tx, event_rx) = mpsc::channel();
        let name = Arc::new(Mutex::new("Speakers".to_string()));
        let backend = SwapBackend {
            name: Arc::clone(&name),
            producer: Arc::new(Mutex::new(Vec::new())),
        };

        let capture = MeetingCapture::start(
            dir.path().to_path_buf(),
            None,
            SystemSource::Custom(Box::new(backend)),
            event_tx,
        )
        .expect("capture starts");

        assert!(wait_for(|| {
            event_rx.try_iter().any(|e| {
                matches!(
                    e,
                    MeetingCaptureEvent::System(SystemAudioEvent::Attached { .. })
                )
            })
        }));

        *name.lock().unwrap() = "Headset".to_string();

        assert!(wait_for(|| {
            let drained: Vec<_> = event_rx.try_iter().collect();
            drained.iter().any(|e| {
                matches!(
                    e,
                    MeetingCaptureEvent::System(SystemAudioEvent::Detached {
                        reason: DetachReason::DefaultDeviceChanged
                    })
                )
            }) && drained.iter().any(|e| {
                matches!(
                    e,
                    MeetingCaptureEvent::System(SystemAudioEvent::Attached {
                        device_name,
                        after_gap: Some(_)
                    }) if device_name == "Headset"
                )
            })
        }));

        capture.stop();
    }

    #[test]
    fn live_chunks_emit_ahead_of_the_block_seal() {
        // FR-009-15: with `live_chunk_samples` set, frames arrive as ~chunk-
        // sized events carrying the meeting-clock offset — while the 60 s
        // block is still filling.
        let dir = tempfile::tempdir().unwrap();
        let (event_tx, event_rx) = mpsc::channel();
        let mic_cb: Arc<Mutex<Option<AudioFrameCallback>>> = Arc::new(Mutex::new(None));
        let tap = {
            let mic_cb = Arc::clone(&mic_cb);
            MicTap::new(
                move |cb| {
                    *mic_cb.lock().unwrap() = Some(cb);
                    Ok(7)
                },
                |_| {},
            )
        };

        let mut config = CaptureConfig {
            block_samples: 60_000, // big enough that no block seals here
            live_chunk_samples: 1_600,
            ..CaptureConfig::default()
        };
        config.time_base = Instant::now();
        let capture = MeetingCapture::start_with_config(
            dir.path().to_path_buf(),
            Some(tap),
            SystemSource::Disabled,
            event_tx,
            config,
        )
        .expect("capture starts");

        let cb = Arc::clone(&mic_cb);
        assert!(wait_for(|| cb.lock().unwrap().is_some()));
        let cb = cb.lock().unwrap().clone().unwrap();
        // 4 000 samples ≈ 2.5 chunks → two live chunks, zero sealed blocks.
        for _ in 0..8 {
            cb(&[0.5f32; 500]);
        }

        // `try_iter` consumes — accumulate what each poll saw.
        let seen = Arc::new(Mutex::new(Vec::new()));
        let drained = Arc::clone(&seen);
        assert!(wait_for(|| {
            drained.lock().unwrap().extend(event_rx.try_iter());
            drained
                .lock()
                .unwrap()
                .iter()
                .filter(|e| matches!(e, MeetingCaptureEvent::LiveChunk { .. }))
                .count()
                >= 2
        }));
        let events = seen.lock().unwrap();
        let chunk_offsets: Vec<u64> = events
            .iter()
            .filter_map(|e| match e {
                MeetingCaptureEvent::LiveChunk {
                    start_offset_ms, ..
                } => Some(*start_offset_ms),
                _ => None,
            })
            .collect();
        // Chunk anchors sit on the block sample axis: the offset is the
        // enclosing block's start stamp plus the intra-block position —
        // so chunk 1 lands exactly one chunk-duration (1 600 samples =
        // 100 ms) after chunk 0 regardless of test timing.
        assert_eq!(chunk_offsets.len(), 2, "chunks: {chunk_offsets:?}");
        // Chunk anchors sit on the block sample axis: chunk 1 lands one
        // chunk-duration (1 600 samples = 100 ms) after chunk 0. Per-segment
        // ms flooring (500 samples = 31.25 ms) sheds ~1 ms across the frame
        // boundaries — well under the coverage sliver floor.
        let delta = chunk_offsets[1].saturating_sub(chunk_offsets[0]);
        assert!((99..=101).contains(&delta), "chunks: {chunk_offsets:?}");
        assert!(chunk_offsets[0] < 100);
        assert!(
            events
                .iter()
                .all(|e| !matches!(e, MeetingCaptureEvent::BlockSealed(_))),
            "no block seals before 60 000 samples arrive"
        );

        capture.stop();
    }

    #[test]
    fn live_chunk_offsets_follow_the_block_axis_across_a_gap() {
        // Push one chunk, go quiet, push the next. A capture gap emits no
        // samples, so the block axis compresses it — the second chunk must
        // land exactly one chunk-duration (100 ms) after the first, on the
        // same timeline its sealing block will use. Wall-clock anchoring
        // would jump ~400 ms and drift the chunk's coverage off the
        // block's, duplicating the speech on both passes.
        let dir = tempfile::tempdir().unwrap();
        let (event_tx, event_rx) = mpsc::channel();
        let mic_cb: Arc<Mutex<Option<AudioFrameCallback>>> = Arc::new(Mutex::new(None));
        let tap = {
            let mic_cb = Arc::clone(&mic_cb);
            MicTap::new(
                move |cb| {
                    *mic_cb.lock().unwrap() = Some(cb);
                    Ok(7)
                },
                |_| {},
            )
        };

        let mut config = CaptureConfig {
            block_samples: 60_000,
            live_chunk_samples: 1_600,
            ..CaptureConfig::default()
        };
        config.time_base = Instant::now();
        let capture = MeetingCapture::start_with_config(
            dir.path().to_path_buf(),
            Some(tap),
            SystemSource::Disabled,
            event_tx,
            config,
        )
        .expect("capture starts");

        let cb = Arc::clone(&mic_cb);
        assert!(wait_for(|| cb.lock().unwrap().is_some()));
        let cb = cb.lock().unwrap().clone().unwrap();
        cb(&[0.5f32; 1_600]); // exactly one chunk
        std::thread::sleep(Duration::from_millis(400));
        cb(&[0.5f32; 1_600]);

        let seen = Arc::new(Mutex::new(Vec::new()));
        let drained = Arc::clone(&seen);
        assert!(wait_for(|| {
            drained.lock().unwrap().extend(event_rx.try_iter());
            drained
                .lock()
                .unwrap()
                .iter()
                .filter(|e| matches!(e, MeetingCaptureEvent::LiveChunk { .. }))
                .count()
                >= 2
        }));
        let chunk_offsets: Vec<u64> = seen
            .lock()
            .unwrap()
            .iter()
            .filter_map(|e| match e {
                MeetingCaptureEvent::LiveChunk {
                    start_offset_ms, ..
                } => Some(*start_offset_ms),
                _ => None,
            })
            .collect();
        assert_eq!(
            chunk_offsets[1].saturating_sub(chunk_offsets[0]),
            100,
            "the gap compresses on the block axis — the second chunk keeps its sample position: {chunk_offsets:?}"
        );

        capture.stop();
    }

    #[test]
    fn live_chunk_offsets_agree_with_their_blocks_across_a_seal() {
        // The dedup contract: a chunk's offset must equal `enclosing
        // block.start_offset_ms + intra-block position * 1000 / rate`.
        // With 200 ms blocks and 100 ms chunks, chunk 2 straddles the
        // seal — it must anchor on block 2's stamp, not extrapolate
        // block 1's.
        let dir = tempfile::tempdir().unwrap();
        let (event_tx, event_rx) = mpsc::channel();
        let mic_cb: Arc<Mutex<Option<AudioFrameCallback>>> = Arc::new(Mutex::new(None));
        let tap = {
            let mic_cb = Arc::clone(&mic_cb);
            MicTap::new(
                move |cb| {
                    *mic_cb.lock().unwrap() = Some(cb);
                    Ok(7)
                },
                |_| {},
            )
        };

        let mut config = CaptureConfig {
            block_samples: 3_200,      // 200 ms
            live_chunk_samples: 1_600, // 100 ms
            ..CaptureConfig::default()
        };
        config.time_base = Instant::now();
        let capture = MeetingCapture::start_with_config(
            dir.path().to_path_buf(),
            Some(tap),
            SystemSource::Disabled,
            event_tx,
            config,
        )
        .expect("capture starts");

        let cb = Arc::clone(&mic_cb);
        assert!(wait_for(|| cb.lock().unwrap().is_some()));
        let cb = cb.lock().unwrap().clone().unwrap();
        // Two feeds with a stall between: block 2's start stamp differs
        // from block 1's, so chunk 2 anchoring on block 2 is observable —
        // block-1 extrapolation would stamp it 200 ms out instead.
        cb(&[0.5f32; 3_200]);
        std::thread::sleep(Duration::from_millis(60));
        cb(&[0.5f32; 3_200]);

        let seen = Arc::new(Mutex::new(Vec::new()));
        let drained = Arc::clone(&seen);
        assert!(wait_for(|| {
            drained.lock().unwrap().extend(event_rx.try_iter());
            let drained = drained.lock().unwrap();
            drained
                .iter()
                .filter(|e| matches!(e, MeetingCaptureEvent::LiveChunk { .. }))
                .count()
                >= 4
                && drained
                    .iter()
                    .filter(|e| matches!(e, MeetingCaptureEvent::BlockSealed(_)))
                    .count()
                    >= 2
        }));
        let events = seen.lock().unwrap();
        let chunk_offsets: Vec<u64> = events
            .iter()
            .filter_map(|e| match e {
                MeetingCaptureEvent::LiveChunk {
                    start_offset_ms, ..
                } => Some(*start_offset_ms),
                _ => None,
            })
            .collect();
        let block_offsets: Vec<u64> = events
            .iter()
            .filter_map(|e| match e {
                MeetingCaptureEvent::BlockSealed(block) => Some(block.start_offset_ms),
                _ => None,
            })
            .collect();
        assert_eq!(chunk_offsets.len(), 4, "chunks: {chunk_offsets:?}");
        assert_eq!(block_offsets.len(), 2, "blocks: {block_offsets:?}");
        // Chunks 0/1 sit on block 1's stamp; chunks 2/3 on block 2's —
        // the seal re-anchors rather than extrapolating block 1.
        assert_eq!(chunk_offsets[0], block_offsets[0]);
        assert_eq!(
            chunk_offsets[1].saturating_sub(block_offsets[0]),
            100,
            "intra-block position maps sample → ms"
        );
        assert_eq!(chunk_offsets[2], block_offsets[1]);
        assert_eq!(chunk_offsets[3].saturating_sub(block_offsets[1]), 100);

        capture.stop();
    }

    #[test]
    fn live_chunk_offsets_track_blocks_when_frames_straddle_a_seal() {
        // Same contract as the aligned test but with frames that do NOT
        // divide the block: 2400-sample frames against 3200-sample blocks,
        // so one frame's tail lands in the next block. The ledger must
        // split the segment at the seal and re-anchor on block 2's stamp —
        // extrapolating block 1 past the boundary would mis-stamp it.
        let dir = tempfile::tempdir().unwrap();
        let (event_tx, event_rx) = mpsc::channel();
        let mic_cb: Arc<Mutex<Option<AudioFrameCallback>>> = Arc::new(Mutex::new(None));
        let tap = {
            let mic_cb = Arc::clone(&mic_cb);
            MicTap::new(
                move |cb| {
                    *mic_cb.lock().unwrap() = Some(cb);
                    Ok(7)
                },
                |_| {},
            )
        };

        let mut config = CaptureConfig {
            block_samples: 3_200,      // 200 ms
            live_chunk_samples: 1_600, // 100 ms
            ..CaptureConfig::default()
        };
        config.time_base = Instant::now();
        let capture = MeetingCapture::start_with_config(
            dir.path().to_path_buf(),
            Some(tap),
            SystemSource::Disabled,
            event_tx,
            config,
        )
        .expect("capture starts");

        let cb = Arc::clone(&mic_cb);
        assert!(wait_for(|| cb.lock().unwrap().is_some()));
        let cb = cb.lock().unwrap().clone().unwrap();
        // Frame 2 straddles the seal: its first 800 samples close block 1,
        // the rest opens block 2. Frame 3 seals block 2.
        cb(&[0.5f32; 2_400]);
        std::thread::sleep(Duration::from_millis(60));
        cb(&[0.5f32; 2_400]);
        cb(&[0.5f32; 1_600]);

        let seen = Arc::new(Mutex::new(Vec::new()));
        let drained = Arc::clone(&seen);
        assert!(wait_for(|| {
            drained.lock().unwrap().extend(event_rx.try_iter());
            let drained = drained.lock().unwrap();
            drained
                .iter()
                .filter(|e| matches!(e, MeetingCaptureEvent::LiveChunk { .. }))
                .count()
                >= 4
                && drained
                    .iter()
                    .filter(|e| matches!(e, MeetingCaptureEvent::BlockSealed(_)))
                    .count()
                    >= 2
        }));
        let events = seen.lock().unwrap();
        let chunk_offsets: Vec<u64> = events
            .iter()
            .filter_map(|e| match e {
                MeetingCaptureEvent::LiveChunk {
                    start_offset_ms, ..
                } => Some(*start_offset_ms),
                _ => None,
            })
            .collect();
        let block_offsets: Vec<u64> = events
            .iter()
            .filter_map(|e| match e {
                MeetingCaptureEvent::BlockSealed(block) => Some(block.start_offset_ms),
                _ => None,
            })
            .collect();
        assert_eq!(chunk_offsets.len(), 4, "chunks: {chunk_offsets:?}");
        assert_eq!(block_offsets.len(), 2, "blocks: {block_offsets:?}");
        assert_eq!(chunk_offsets[0], block_offsets[0]);
        assert_eq!(chunk_offsets[1].saturating_sub(block_offsets[0]), 100);
        // The straddling frame's second half starts block 2 — the chunk
        // anchored there equals the block's own stamp, not block 1 + 200 ms.
        assert_eq!(chunk_offsets[2], block_offsets[1]);
        assert_eq!(chunk_offsets[3].saturating_sub(block_offsets[1]), 100);

        capture.stop();
    }

    #[test]
    fn live_chunk_offsets_survive_a_failed_seal_backlog() {
        // A failed block write leaves `pending` at/above `block_samples`.
        // The ledger must fold that backlog — the failed frame stays in
        // pending and seals on a later push — and anchor the recovered
        // frame on the spillover's boundary stamp, never on a stale
        // block-0 start and never on a zero-length segment.
        let dir = tempfile::tempdir().unwrap();
        let (event_tx, event_rx) = mpsc::channel();
        let mic_cb: Arc<Mutex<Option<AudioFrameCallback>>> = Arc::new(Mutex::new(None));
        let tap = {
            let mic_cb = Arc::clone(&mic_cb);
            MicTap::new(
                move |cb| {
                    *mic_cb.lock().unwrap() = Some(cb);
                    Ok(7)
                },
                |_| {},
            )
        };

        let mut config = CaptureConfig {
            block_samples: 3_200,      // 200 ms
            live_chunk_samples: 1_600, // 100 ms
            ..CaptureConfig::default()
        };
        config.time_base = Instant::now();
        let capture = MeetingCapture::start_with_config(
            dir.path().to_path_buf(),
            Some(tap),
            SystemSource::Disabled,
            event_tx,
            config,
        )
        .expect("capture starts");

        // First seal fails: `mic-0001.wav` as a directory is unwritable.
        std::fs::create_dir(dir.path().join("mic-0001.wav")).unwrap();

        let cb = Arc::clone(&mic_cb);
        assert!(wait_for(|| cb.lock().unwrap().is_some()));
        let cb = cb.lock().unwrap().clone().unwrap();
        cb(&[0.5f32; 3_200]); // exactly fills a block → the seal fails

        let seen = Arc::new(Mutex::new(Vec::new()));
        let drained = Arc::clone(&seen);
        assert!(wait_for(|| {
            drained.lock().unwrap().extend(event_rx.try_iter());
            drained
                .lock()
                .unwrap()
                .iter()
                .any(|e| matches!(e, MeetingCaptureEvent::WriteFailed { .. }))
        }));

        // The writer recovers on the next push once the path is writable.
        std::fs::remove_dir(dir.path().join("mic-0001.wav")).unwrap();
        cb(&[0.5f32; 2_400]); // seals the backlog block, spills 400
        cb(&[0.5f32; 1_600]); // seals block 2

        assert!(wait_for(|| {
            drained.lock().unwrap().extend(event_rx.try_iter());
            let drained = drained.lock().unwrap();
            drained
                .iter()
                .filter(|e| matches!(e, MeetingCaptureEvent::LiveChunk { .. }))
                .count()
                >= 2
                && drained
                    .iter()
                    .filter(|e| matches!(e, MeetingCaptureEvent::BlockSealed(_)))
                    .count()
                    >= 2
        }));
        let events = seen.lock().unwrap();
        let chunk_offsets: Vec<u64> = events
            .iter()
            .filter_map(|e| match e {
                MeetingCaptureEvent::LiveChunk {
                    start_offset_ms, ..
                } => Some(*start_offset_ms),
                _ => None,
            })
            .collect();
        let block_offsets: Vec<u64> = events
            .iter()
            .filter_map(|e| match e {
                MeetingCaptureEvent::BlockSealed(block) => Some(block.start_offset_ms),
                _ => None,
            })
            .collect();
        // The backlog block keeps the first frame's stamp; the frame that
        // recovered it anchors its chunks at the *second* block's stamp —
        // the boundary `now`, not block 1's (stale) start.
        assert_eq!(block_offsets.len(), 2, "blocks: {block_offsets:?}");
        assert_eq!(chunk_offsets.len(), 2, "chunks: {chunk_offsets:?}");
        assert!(
            block_offsets[1] > block_offsets[0],
            "the recovery push stamps a later boundary: {block_offsets:?}"
        );
        assert_eq!(chunk_offsets[0], block_offsets[1]);
        assert_eq!(
            chunk_offsets[1].saturating_sub(block_offsets[1]),
            100,
            "chunks: {chunk_offsets:?} blocks: {block_offsets:?}"
        );
        // Ordering: the WriteFailed from the failed seal precedes the
        // blocks that the recovery push went on to seal.
        let write_failed_at = events
            .iter()
            .position(|e| matches!(e, MeetingCaptureEvent::WriteFailed { .. }))
            .expect("the failed seal is reported");
        let first_seal_at = events
            .iter()
            .position(|e| matches!(e, MeetingCaptureEvent::BlockSealed(_)))
            .expect("blocks seal after recovery");
        assert!(write_failed_at < first_seal_at);

        capture.stop();
    }
}
