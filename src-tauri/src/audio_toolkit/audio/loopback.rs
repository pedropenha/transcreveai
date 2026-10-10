//! System-audio ("loopback") capture for the meeting notetaker
//! (FR-009-03/04, AC-009-05): the `system` track of a meeting records what the
//! user hears — the other call participants — by tapping the default output
//! endpoint.
//!
//! On Windows, cpal's WASAPI backend transparently sets
//! `AUDCLNT_STREAMFLAGS_LOOPBACK` when an input stream is built on a render
//! (`eRender`) device, so no extra dependency is needed: open an *input*
//! stream on the *output* device using its mix format
//! (`default_output_config`). On other platforms `open` simply fails and the
//! meeting proceeds with the mic track only.
//!
//! FR-009-04 requires reattaching within ≤ 2 s when the default render
//! endpoint changes (e.g. plugging in a Bluetooth headset). cpal exposes no
//! `IMMNotificationClient`, so the worker polls `default_device_name` every
//! [`POLL_INTERVAL`] and also honors the cpal stream error callback —
//! together they bound the worst-case detection to ~250 ms plus reopen.
//!
//! The device boundary sits behind [`LoopbackBackend`]/[`LoopbackStream`] so
//! the attach/drain/detach state machine is unit-testable with a scripted
//! fake; only [`CpalLoopbackBackend`] touches real hardware.

use std::{
    sync::{atomic::Ordering, mpsc, Arc, Mutex},
    thread::JoinHandle,
    time::{Duration, Instant},
};

use cpal::{
    traits::{DeviceTrait, HostTrait, StreamTrait},
    Sample, SizedSample,
};
use rtrb::{Consumer, RingBuffer};

use super::{
    recorder::{drain_available_samples, AudioRecorder, CaptureTransportState},
    AudioFrameCallback, FrameResampler,
};
use crate::audio_toolkit::constants;

#[cfg(target_os = "macos")]
mod mac;

/// How often the worker re-checks the default render endpoint. With the
/// 100 ms first reattach delay, a device swap reattaches well inside the
/// FR-009-04 budget of 2 s.
const POLL_INTERVAL: Duration = Duration::from_millis(250);
/// Idle wake cadence while the ring is empty — the same poll the mic
/// consumer uses, so newly-arriving audio is drained within a few ms. While
/// samples are queued the worker never sleeps: real-time input must not
/// outrun the drain.
const RING_IDLE_POLL: Duration = Duration::from_millis(10);
/// Ring capacity, mirroring the microphone path.
const RING_SECONDS: usize = 2;
/// Upper bound per drain so a stalled worker cannot fall arbitrarily behind.
const MAX_DRAIN_CHUNK: Duration = Duration::from_millis(50);
/// Frame size delivered to consumers — same 30 ms frames the mic path emits.
const FRAME_DURATION: Duration = Duration::from_millis(30);

/// Why a loopback stream was dropped.
#[derive(Clone, Debug, PartialEq)]
pub enum DetachReason {
    /// `GetDefaultAudioEndpoint` now names a different render endpoint (or
    /// none). The audio between `Detached` and the next `Attached` is a gap
    /// the session marks with a `gap_marker` segment (FR-009-04).
    DefaultDeviceChanged,
    /// cpal's stream error callback fired (e.g. `AUDCLNT_E_DEVICE_INVALIDATED`
    /// when the endpoint is unplugged or goes exclusive).
    StreamError(String),
    /// Opening the current default endpoint failed — the worker keeps
    /// retrying with [`reattach_delay`] backoff until it succeeds or stops.
    OpenFailed(String),
}

/// Lifecycle events the meeting session needs to build `gap_marker`s and
/// surface "the other participants are not being captured" warnings.
#[derive(Clone, Debug, PartialEq)]
pub enum SystemAudioEvent {
    /// Capture is running on `device_name`. `after_gap` is the measured
    /// outage when this attach follows a `Detached`.
    Attached {
        device_name: String,
        after_gap: Option<Duration>,
    },
    /// Capture stopped producing frames until the next `Attached`.
    Detached { reason: DetachReason },
    /// The endpoint callback could not fit every sample into the ring and
    /// dropped some — the system track is missing audio even though the
    /// stream stayed attached. `dropped_samples` is the cumulative count for
    /// the current attach; a reattach resets it to zero.
    Overrun { dropped_samples: u64 },
}

pub type SystemAudioEventCallback = Arc<dyn Fn(SystemAudioEvent) + Send + Sync + 'static>;

/// One open loopback stream. Dropping it ends the capture. No `Send` bound:
/// the stream is opened and drained on the capture worker thread and never
/// leaves it — and `cpal::Stream` is not `Send` on CoreAudio.
pub trait LoopbackStream {
    /// Name of the render endpoint this stream is bound to.
    fn device_name(&self) -> &str;
    /// Native sample rate of the stream (mono-averaged by the callback).
    fn sample_rate(&self) -> u32;
    /// `Some(message)` once the backend's error callback reported the stream
    /// can no longer capture; `None` while healthy.
    fn failure(&self) -> Option<String>;
    /// Samples the capture callback dropped because the ring was full —
    /// cumulative for this stream's lifetime. Backends that cannot observe
    /// overruns report zero.
    fn dropped_samples(&self) -> u64 {
        0
    }
}

/// The two halves of an opened capture: the live stream plus the consumer
/// end of its sample ring.
pub struct OpenedLoopback {
    pub stream: Box<dyn LoopbackStream>,
    pub samples: Consumer<f32>,
}

/// Source of loopback captures over the *current default* render endpoint.
/// [`CpalLoopbackBackend`] is the production implementation; tests inject a
/// scripted backend so the worker's attach/detach/backoff logic runs without
/// hardware.
pub trait LoopbackBackend: Send {
    /// Name of the current default render endpoint: `Ok(None)` when there
    /// is none, `Err` when the probe itself failed. The distinction matters
    /// — a transient `name()` error must not detach a healthy stream.
    fn default_device_name(&self) -> Result<Option<String>, String>;
    /// Open a loopback capture on the current default render endpoint.
    fn open(&self) -> Result<OpenedLoopback, String>;
}

/// Backoff between reattach attempts: 100 ms, doubling to a 1.6 s cap. The
/// first retry alone lands a device swap inside the FR-009-04 ≤ 2 s budget,
/// and the schedule stays cheap if the endpoint keeps failing (e.g. an
/// endpoint that went exclusive-mode to another app).
pub(crate) fn reattach_delay(attempt: u32) -> Duration {
    Duration::from_millis(100u64.saturating_mul(1 << attempt.min(4)))
}

/// Pure detach policy, separated from the poll loop for testing: a stream is
/// detached when its backend reported a failure or when the default render
/// endpoint no longer matches the device the stream is bound to. A failed
/// *probe* (`Err` from `default_device_name`) is not evidence of a swap —
/// the stream stays attached rather than flapping on a transient `name()`
/// error, and the next successful poll still sees a real change.
pub(crate) fn assess_stream(
    failure: Option<String>,
    bound_name: &str,
    current_default: Result<Option<String>, String>,
) -> Option<DetachReason> {
    if let Some(message) = failure {
        return Some(DetachReason::StreamError(message));
    }
    match current_default {
        Err(err) => {
            log::debug!("Default endpoint probe failed ({err}) — keeping the loopback stream");
            None
        }
        Ok(current) if current.as_deref() != Some(bound_name) => {
            Some(DetachReason::DefaultDeviceChanged)
        }
        Ok(_) => None,
    }
}

/// Handle to the system-audio capture worker. `stop`/`drop` ends the stream
/// and joins the thread.
pub struct SystemAudioCapture {
    stop_tx: Option<mpsc::Sender<()>>,
    worker: Option<JoinHandle<()>>,
}

impl SystemAudioCapture {
    /// Start capturing the default render endpoint. `frame_cb` receives 16 kHz
    /// mono frames on the worker thread — keep it cheap (forward to a
    /// channel). `event_cb` receives [`SystemAudioEvent`]s for gap markers and
    /// warnings.
    pub fn start(
        frame_cb: AudioFrameCallback,
        event_cb: SystemAudioEventCallback,
    ) -> Result<Self, String> {
        Self::start_with_backend(default_backend(), POLL_INTERVAL, frame_cb, event_cb)
    }

    /// Same as [`Self::start`] with an injected backend and poll cadence — the
    /// seam the worker tests drive.
    pub(crate) fn start_with_backend(
        backend: Box<dyn LoopbackBackend>,
        poll_interval: Duration,
        frame_cb: AudioFrameCallback,
        event_cb: SystemAudioEventCallback,
    ) -> Result<Self, String> {
        let (stop_tx, stop_rx) = mpsc::channel();
        let worker = std::thread::Builder::new()
            .name("system-audio-capture".into())
            .spawn(move || run_loopback_worker(backend, stop_rx, frame_cb, event_cb, poll_interval))
            .map_err(|e| format!("Failed to spawn the loopback capture thread: {e}"))?;
        Ok(Self {
            stop_tx: Some(stop_tx),
            worker: Some(worker),
        })
    }

    /// Signal the worker to flush its resampler tail and stop, then join it.
    pub fn stop(mut self) {
        self.signal_stop();
    }

    fn signal_stop(&mut self) {
        if let Some(tx) = self.stop_tx.take() {
            let _ = tx.send(());
        }
        if let Some(handle) = self.worker.take() {
            if let Err(e) = handle.join() {
                log::error!("System audio capture worker panicked: {e:?}");
            }
        }
    }
}

impl Drop for SystemAudioCapture {
    fn drop(&mut self) {
        self.signal_stop();
    }
}

/// Why [`run_attached`] returned.
enum AttachedExit {
    /// The worker was asked to stop; the capture session is over.
    Stop,
    /// The stream can no longer be trusted — detach and reattach.
    Detach(DetachReason),
}

/// The worker's steady state: drain the ring into 16 kHz frames until asked
/// to stop or [`assess_stream`] reports the stream is no longer bound to the
/// default endpoint. A resampler is built per attach so a different device
/// rate never feeds a stale filter chain.
///
/// Draining mirrors the microphone consumer: while the ring holds samples
/// the loop never sleeps — it drains bounded chunks back-to-back, checking
/// the stop signal between them. Sleeping once per [`POLL_INTERVAL`] here is
/// what let continuous input outrun the old 50 ms-per-250 ms drain and
/// overflow the 2 s ring. Endpoint health is still assessed only once per
/// `poll_interval`; overrun drops surface as [`SystemAudioEvent::Overrun`].
fn run_attached(
    opened: OpenedLoopback,
    backend: &dyn LoopbackBackend,
    stop_rx: &mpsc::Receiver<()>,
    frame_cb: &AudioFrameCallback,
    event_cb: &SystemAudioEventCallback,
    poll_interval: Duration,
) -> AttachedExit {
    let OpenedLoopback {
        stream,
        mut samples,
    } = opened;
    let sample_rate = stream.sample_rate();
    let mut resampler = match FrameResampler::new(
        sample_rate as usize,
        constants::WHISPER_SAMPLE_RATE as usize,
        FRAME_DURATION,
    ) {
        Ok(resampler) => resampler,
        Err(err) => {
            return AttachedExit::Detach(DetachReason::OpenFailed(format!(
                "Failed to initialize the loopback resampler: {err}"
            )))
        }
    };
    let max_drain_samples =
        ((sample_rate as u128 * MAX_DRAIN_CHUNK.as_millis()) / 1_000).max(1) as usize;

    let mut next_assessment = Instant::now();
    let mut dropped_seen = stream.dropped_samples();

    // Final drain: deliver every sample still in the ring before the
    // resampler tail flush. Bounded by the ring's capacity plus one drain
    // chunk — the producer may keep feeding a live stream while we drain,
    // so "while non-empty" alone could outrun the bound.
    let drain_all = |samples: &mut Consumer<f32>,
                     resampler: &mut FrameResampler,
                     frame_cb: &AudioFrameCallback| {
        let mut budget = samples.slots() + max_drain_samples;
        while samples.slots() > 0 && budget > 0 {
            let drained = drain_available_samples(samples, max_drain_samples, |raw| {
                resampler.push(raw, |frame| frame_cb(frame));
            });
            budget = budget.saturating_sub(drained.max(1));
        }
    };

    // Overruns are only checked on the assessment cadence, so drops in the
    // last poll window would vanish — report the final reading before any
    // exit so the meeting's cumulative loss count stays exact.
    let report_overrun = |stream: &dyn LoopbackStream, dropped_seen: &mut u64| {
        let dropped = stream.dropped_samples();
        if dropped > *dropped_seen {
            log::warn!(
                "System audio ring dropped {} samples ({} total); input outran the drain",
                dropped - *dropped_seen,
                dropped
            );
            *dropped_seen = dropped;
            // Emit on every growth — the session accumulates the deltas,
            // so suppressing later reports would undercount the loss.
            event_cb(SystemAudioEvent::Overrun {
                dropped_samples: dropped,
            });
        }
    };

    loop {
        // Stop is observed between bounded chunks so it cannot sit behind a
        // full ring; when the ring is empty the worker blocks briefly and the
        // stop signal wakes it immediately.
        let stop_requested = if samples.slots() > 0 {
            matches!(
                stop_rx.try_recv(),
                Ok(()) | Err(mpsc::TryRecvError::Disconnected)
            )
        } else {
            match stop_rx.recv_timeout(RING_IDLE_POLL) {
                Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => true,
                Err(mpsc::RecvTimeoutError::Timeout) => false,
            }
        };
        if stop_requested {
            // Drain whatever the callback still queued before flushing the
            // resampler tail, so stopping loses only audio that never
            // arrived.
            drain_all(&mut samples, &mut resampler, frame_cb);
            resampler.finish(|frame| frame_cb(frame));
            report_overrun(stream.as_ref(), &mut dropped_seen);
            return AttachedExit::Stop;
        }

        drain_available_samples(&mut samples, max_drain_samples, |raw| {
            resampler.push(raw, |frame| frame_cb(frame));
        });

        let now = Instant::now();
        if now >= next_assessment {
            next_assessment = now + poll_interval;

            if let Some(reason) = assess_stream(
                stream.failure(),
                stream.device_name(),
                backend.default_device_name(),
            ) {
                // Drain the ring, then flush the resampler tail, so audio
                // that already arrived is delivered ahead of the gap.
                drain_all(&mut samples, &mut resampler, frame_cb);
                resampler.finish(|frame| frame_cb(frame));
                report_overrun(stream.as_ref(), &mut dropped_seen);
                return AttachedExit::Detach(reason);
            }

            report_overrun(stream.as_ref(), &mut dropped_seen);
        }
    }
}

/// Outer attach/reattach loop: open the current default endpoint, run it,
/// and on detach back off with [`reattach_delay`] and reopen — indefinitely,
/// so a transient outage (endpoint renegotiating, exclusive grab released)
/// self-heals for the life of the meeting.
fn run_loopback_worker(
    backend: Box<dyn LoopbackBackend>,
    stop_rx: mpsc::Receiver<()>,
    frame_cb: AudioFrameCallback,
    event_cb: SystemAudioEventCallback,
    poll_interval: Duration,
) {
    let mut attempts: u32 = 0;
    let mut gap_started_at: Option<Instant> = None;

    loop {
        match backend.open() {
            Ok(opened) => {
                let device_name = opened.stream.device_name().to_string();
                event_cb(SystemAudioEvent::Attached {
                    device_name,
                    after_gap: gap_started_at.take().map(|start| start.elapsed()),
                });
                attempts = 0;
                match run_attached(
                    opened,
                    backend.as_ref(),
                    &stop_rx,
                    &frame_cb,
                    &event_cb,
                    poll_interval,
                ) {
                    AttachedExit::Stop => return,
                    AttachedExit::Detach(reason) => {
                        gap_started_at = Some(Instant::now());
                        event_cb(SystemAudioEvent::Detached { reason });
                    }
                }
            }
            Err(message) => {
                if gap_started_at.is_none() {
                    gap_started_at = Some(Instant::now());
                    event_cb(SystemAudioEvent::Detached {
                        reason: DetachReason::OpenFailed(message.clone()),
                    });
                }
                attempts = attempts.saturating_add(1);
                // Wait out the backoff, but leave immediately on stop.
                match stop_rx.recv_timeout(reattach_delay(attempts)) {
                    Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => return,
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                }
            }
        }
    }
}

/// The platform's production backend: WASAPI loopback on Windows,
/// ScreenCaptureKit on macOS, the (always-failing) cpal path elsewhere.
#[cfg(windows)]
pub(crate) fn default_backend() -> Box<dyn LoopbackBackend> {
    Box::new(CpalLoopbackBackend)
}

#[cfg(target_os = "macos")]
pub(crate) fn default_backend() -> Box<dyn LoopbackBackend> {
    Box::new(mac::SckLoopbackBackend)
}

#[cfg(not(any(windows, target_os = "macos")))]
pub(crate) fn default_backend() -> Box<dyn LoopbackBackend> {
    Box::new(CpalLoopbackBackend)
}

/// Production [`LoopbackBackend`]: a cpal input stream over the default
/// *output* device, which the WASAPI backend runs in loopback mode.
pub struct CpalLoopbackBackend;

impl LoopbackBackend for CpalLoopbackBackend {
    fn default_device_name(&self) -> Result<Option<String>, String> {
        match crate::audio_toolkit::get_cpal_host().default_output_device() {
            // A `name()` failure here is a probe error, not "no device" —
            // swallowing it as `None` would read as a device swap and flap
            // the capture on every transient failure.
            Some(device) => device
                .name()
                .map(Some)
                .map_err(|e| format!("Failed to name the default output device: {e}")),
            None => Ok(None),
        }
    }

    fn open(&self) -> Result<OpenedLoopback, String> {
        let host = crate::audio_toolkit::get_cpal_host();
        let device = host
            .default_output_device()
            .ok_or_else(|| "No default output device found".to_string())?;
        // A nameless device would bind the stream to "" and mismatch every
        // later `default_device_name` poll — detach/reattach flapping. Treat
        // it as an open failure so the backoff retry handles it instead.
        let device_name = device
            .name()
            .map_err(|e| format!("Failed to name the default output device: {e}"))?;
        // `default_output_config` returns the endpoint's mix format — the only
        // format WASAPI accepts for a shared-mode loopback stream.
        let config = device
            .default_output_config()
            .map_err(|e| format!("Failed to fetch the output config for loopback: {e}"))?;
        let failure = Arc::new(Mutex::new(None));
        let transport = Arc::new(CaptureTransportState::default());
        let (stream, samples) = match config.sample_format() {
            cpal::SampleFormat::U8 => {
                build_loopback_stream::<u8>(&device, &config, &failure, &transport)
            }
            cpal::SampleFormat::I8 => {
                build_loopback_stream::<i8>(&device, &config, &failure, &transport)
            }
            cpal::SampleFormat::I16 => {
                build_loopback_stream::<i16>(&device, &config, &failure, &transport)
            }
            cpal::SampleFormat::I32 => {
                build_loopback_stream::<i32>(&device, &config, &failure, &transport)
            }
            cpal::SampleFormat::F32 => {
                build_loopback_stream::<f32>(&device, &config, &failure, &transport)
            }
            sample_format => {
                return Err(format!(
                    "Unsupported loopback sample format: {sample_format:?}"
                ))
            }
        }
        .map_err(|e| format!("Failed to build the loopback stream: {e}"))?;
        stream
            .play()
            .map_err(|e| format!("Failed to start the loopback stream: {e}"))?;
        Ok(OpenedLoopback {
            stream: Box::new(CpalLoopbackStream {
                _stream: stream,
                device_name,
                sample_rate: config.sample_rate().0,
                failure,
                transport,
            }),
            samples,
        })
    }
}

/// cpal-backed [`LoopbackStream`]; dropping it tears down the stream.
struct CpalLoopbackStream {
    _stream: cpal::Stream,
    device_name: String,
    sample_rate: u32,
    failure: Arc<Mutex<Option<String>>>,
    transport: Arc<CaptureTransportState>,
}

impl LoopbackStream for CpalLoopbackStream {
    fn device_name(&self) -> &str {
        &self.device_name
    }

    fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    fn failure(&self) -> Option<String> {
        self.failure.lock().ok().and_then(|slot| slot.clone())
    }

    fn dropped_samples(&self) -> u64 {
        self.transport.overrun_samples.load(Ordering::Relaxed)
    }
}

/// Build the cpal input stream on a render device (WASAPI loopback) and the
/// SPSC ring it feeds. Mono averaging and overrun accounting reuse
/// [`AudioRecorder::write_input_to_ring`] so both tracks behave identically.
fn build_loopback_stream<T>(
    device: &cpal::Device,
    config: &cpal::SupportedStreamConfig,
    failure: &Arc<Mutex<Option<String>>>,
    transport: &Arc<CaptureTransportState>,
) -> Result<(cpal::Stream, Consumer<f32>), cpal::BuildStreamError>
where
    T: Sample + SizedSample + Copy + Send + 'static,
    f32: cpal::FromSample<T>,
{
    let channels = config.channels() as usize;
    let ring_capacity = config.sample_rate().0 as usize * RING_SECONDS;
    let (mut producer, mut consumer) = RingBuffer::new(ring_capacity);

    // Touch rtrb's uninitialized pages before the stream starts, as the mic
    // path does, to keep first-callback page faults off the audio thread.
    {
        let chunk = producer
            .write_chunk(ring_capacity)
            .expect("new audio ring has its full capacity available");
        chunk.commit_all();
    }
    {
        let chunk = consumer
            .read_chunk(ring_capacity)
            .expect("pre-filled audio ring is readable");
        chunk.commit_all();
    }

    let callback_transport = Arc::clone(transport);
    let data_cb = move |data: &[T], _: &cpal::InputCallbackInfo| {
        AudioRecorder::write_input_to_ring(
            data,
            channels,
            None,
            &mut producer,
            &callback_transport,
        );
    };
    let error_flag = Arc::clone(failure);
    let stream = device.build_input_stream(
        &config.clone().into(),
        data_cb,
        move |err| {
            if let Ok(mut slot) = error_flag.lock() {
                *slot = Some(err.to_string());
            }
        },
        None,
    )?;
    Ok((stream, consumer))
}

#[cfg(test)]
mod tests;
