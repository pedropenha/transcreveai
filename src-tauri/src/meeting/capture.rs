//! Meeting capture composition (FR-009-03..05): the `mic` track taps the one
//! shared microphone stream (`FrameTap::Raw` + `when_idle`, so frames keep
//! flowing between dictation sessions) and the `system` track runs the WASAPI
//! loopback; both feed [`BlockWriter`]s on dedicated writer threads so frame
//! callbacks never wait on disk.
//!
//! This is plumbing, not the session: T-064 owns start/pause/stop policy,
//! duration limits and consent; T-065 consumes the emitted block/lifecycle
//! events for live transcription and `gap_marker`s.

use std::{
    path::PathBuf,
    sync::{mpsc, Arc},
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
}

/// Sealed blocks per track when the capture stops. Recovery never depends on
/// this in-memory list — the directory is the source of truth.
#[derive(Debug, Default)]
pub struct CaptureSummary {
    pub mic: Vec<SealedBlock>,
    pub system: Vec<SealedBlock>,
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
    Stop(mpsc::Sender<Vec<SealedBlock>>),
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
    fn shutdown(self) -> Vec<SealedBlock> {
        let (reply_tx, reply_rx) = mpsc::channel();
        let _ = self.tx.send(TrackMsg::Stop(reply_tx));
        let blocks = reply_rx.recv().unwrap_or_default();
        let _ = self.handle.join();
        blocks
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
    events: mpsc::Sender<MeetingCaptureEvent>,
) -> Result<TrackPipeline> {
    let writer = BlockWriter::with_block_samples(&dir, track, t0, block_samples)?;
    let (tx, rx) = mpsc::channel::<TrackMsg>();
    let handle = std::thread::Builder::new()
        .name(format!("meeting-{}-writer", track.label()))
        .spawn(move || run_track_writer(track, writer, &rx, &events))?;
    Ok(TrackPipeline { tx, handle })
}

fn run_track_writer(
    track: Track,
    mut writer: BlockWriter,
    rx: &mpsc::Receiver<TrackMsg>,
    events: &mpsc::Sender<MeetingCaptureEvent>,
) {
    let emit_newly_sealed = |writer: &BlockWriter, sealed_before: usize| {
        for block in &writer.sealed()[sealed_before..] {
            let _ = events.send(MeetingCaptureEvent::BlockSealed(block.clone()));
        }
    };

    while let Ok(msg) = rx.recv() {
        match msg {
            TrackMsg::Frame(frame) => {
                let sealed_before = writer.sealed().len();
                if let Err(err) = writer.push(&frame, Instant::now()) {
                    let _ = events.send(MeetingCaptureEvent::WriteFailed {
                        track,
                        message: err.to_string(),
                    });
                } else {
                    emit_newly_sealed(&writer, sealed_before);
                }
            }
            TrackMsg::Stop(reply) => {
                let sealed_before = writer.sealed().len();
                if let Err(err) = writer.drain_tail(Instant::now()) {
                    let _ = events.send(MeetingCaptureEvent::WriteFailed {
                        track,
                        message: err.to_string(),
                    });
                }
                emit_newly_sealed(&writer, sealed_before);
                let _ = reply.send(writer.into_sealed());
                return;
            }
        }
    }
    // Sender dropped without Stop: still flush the tail rather than lose it.
    let sealed_before = writer.sealed().len();
    if let Err(err) = writer.drain_tail(Instant::now()) {
        let _ = events.send(MeetingCaptureEvent::WriteFailed {
            track,
            message: err.to_string(),
        });
    }
    emit_newly_sealed(&writer, sealed_before);
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
        Self::start_with_block_samples(
            audio_dir,
            mic,
            system,
            events,
            BLOCK_DURATION.as_secs() as usize * WHISPER_SAMPLE_RATE as usize,
        )
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
        std::fs::create_dir_all(&audio_dir)?;
        // Shared monotonic base: `start_offset_ms` of mic and system blocks
        // are comparable across tracks (FR-009-03).
        let t0 = Instant::now();

        let (mic_writer, mic_subscriber_id, mic_unsubscribe) = match mic {
            Some(tap) => {
                match spawn_track_writer(
                    audio_dir.clone(),
                    Track::Mic,
                    block_samples,
                    t0,
                    events.clone(),
                ) {
                    Ok(pipeline) => {
                        let tx = pipeline.tx.clone();
                        let frame_cb: AudioFrameCallback = Arc::new(move |frame| {
                            let _ = tx.send(TrackMsg::Frame(frame.to_vec()));
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
                    events.clone(),
                ) {
                    Ok(pipeline) => {
                        let tx = pipeline.tx.clone();
                        let frame_cb: AudioFrameCallback = Arc::new(move |frame| {
                            let _ = tx.send(TrackMsg::Frame(frame.to_vec()));
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
        CaptureSummary {
            mic: self
                .mic_writer
                .take()
                .map(TrackPipeline::shutdown)
                .unwrap_or_default(),
            system: self
                .system_writer
                .take()
                .map(TrackPipeline::shutdown)
                .unwrap_or_default(),
        }
    }

    /// Directory holding this capture's blocks.
    pub fn audio_dir(&self) -> &std::path::Path {
        &self.audio_dir
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
        fn default_device_name(&self) -> Option<String> {
            Some("Fake Speakers".to_string())
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
            fn default_device_name(&self) -> Option<String> {
                Some(self.name.lock().unwrap().clone())
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
}
