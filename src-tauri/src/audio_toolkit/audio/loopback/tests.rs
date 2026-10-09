//! Scripted-fake tests for the loopback worker: the real device is unreachable
//! from tests, so [`FakeBackend`] scripts endpoint names, open failures and
//! mid-stream errors while the genuine attach/drain/resample/detach logic runs.

use super::*;
use rtrb::{Producer, RingBuffer};
use std::{
    collections::VecDeque,
    sync::atomic::{AtomicU64, AtomicUsize, Ordering},
    time::Duration,
};

/// Everything a test needs to drive an opened fake stream: push samples into
/// its ring, flip its failure flag and set its overrun counter.
struct FakeHandle {
    producer: Producer<f32>,
    failure: Arc<Mutex<Option<String>>>,
    dropped: Arc<AtomicU64>,
}

struct FakeStream {
    device_name: String,
    sample_rate: u32,
    failure: Arc<Mutex<Option<String>>>,
    dropped: Arc<AtomicU64>,
}

impl LoopbackStream for FakeStream {
    fn device_name(&self) -> &str {
        &self.device_name
    }

    fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    fn failure(&self) -> Option<String> {
        self.failure.lock().unwrap().clone()
    }

    fn dropped_samples(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }
}

struct FakeBackend {
    default_name: Arc<Mutex<Option<String>>>,
    /// One-shot open failure: consumed by the next `open` call.
    open_error: Arc<Mutex<Option<String>>>,
    /// While set, `default_device_name` fails like a transient `name()`
    /// error — the worker must keep the stream rather than flap.
    probe_error: Arc<Mutex<Option<String>>>,
    sample_rate: u32,
    opens: Arc<AtomicUsize>,
    handles: Arc<Mutex<VecDeque<FakeHandle>>>,
}

impl FakeBackend {
    fn new(default_name: &str) -> Self {
        Self {
            default_name: Arc::new(Mutex::new(Some(default_name.to_string()))),
            open_error: Arc::new(Mutex::new(None)),
            probe_error: Arc::new(Mutex::new(None)),
            sample_rate: 16_000,
            opens: Arc::new(AtomicUsize::new(0)),
            handles: Arc::new(Mutex::new(VecDeque::new())),
        }
    }

    fn take_handle(&self) -> Arc<Mutex<VecDeque<FakeHandle>>> {
        Arc::clone(&self.handles)
    }
}

impl LoopbackBackend for FakeBackend {
    fn default_device_name(&self) -> Result<Option<String>, String> {
        if let Some(error) = self.probe_error.lock().unwrap().clone() {
            return Err(error);
        }
        Ok(self.default_name.lock().unwrap().clone())
    }

    fn open(&self) -> Result<OpenedLoopback, String> {
        self.opens.fetch_add(1, Ordering::Relaxed);
        if let Some(error) = self.open_error.lock().unwrap().take() {
            return Err(error);
        }
        let device_name = self
            .default_name
            .lock()
            .unwrap()
            .clone()
            .ok_or_else(|| "No default output device found".to_string())?;
        let (producer, samples) = RingBuffer::new(self.sample_rate as usize * RING_SECONDS);
        let failure = Arc::new(Mutex::new(None));
        let dropped = Arc::new(AtomicU64::new(0));
        self.handles.lock().unwrap().push_back(FakeHandle {
            producer,
            failure: Arc::clone(&failure),
            dropped: Arc::clone(&dropped),
        });
        Ok(OpenedLoopback {
            stream: Box::new(FakeStream {
                device_name,
                sample_rate: self.sample_rate,
                failure,
                dropped,
            }),
            samples,
        })
    }
}

/// Push f32 samples into a ring end.
fn feed(producer: &mut Producer<f32>, samples: &[f32]) {
    let chunk = producer
        .write_chunk_uninit(samples.len())
        .expect("test ring fits the scripted samples");
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

struct Captured {
    frames: Arc<Mutex<Vec<Vec<f32>>>>,
    events: Arc<Mutex<Vec<SystemAudioEvent>>>,
}

fn start(backend: FakeBackend) -> (SystemAudioCapture, Captured, FakeHandles) {
    start_with_poll(backend, Duration::from_millis(5))
}

/// Same as [`start`] with a caller-chosen assessment cadence — the sustained
/// tests run the production [`POLL_INTERVAL`].
fn start_with_poll(
    backend: FakeBackend,
    poll_interval: Duration,
) -> (SystemAudioCapture, Captured, FakeHandles) {
    let captured = Captured {
        frames: Arc::new(Mutex::new(Vec::new())),
        events: Arc::new(Mutex::new(Vec::new())),
    };
    let frames = Arc::clone(&captured.frames);
    let events = Arc::clone(&captured.events);
    let handles = FakeHandles {
        queue: backend.take_handle(),
        opens: Arc::clone(&backend.opens),
        default_name: Arc::clone(&backend.default_name),
        probe_error: Arc::clone(&backend.probe_error),
    };
    let capture = SystemAudioCapture::start_with_backend(
        Box::new(backend),
        poll_interval,
        Arc::new(move |frame| frames.lock().unwrap().push(frame.to_vec())),
        Arc::new(move |event| events.lock().unwrap().push(event)),
    )
    .expect("capture starts");
    (capture, captured, handles)
}

struct FakeHandles {
    queue: Arc<Mutex<VecDeque<FakeHandle>>>,
    opens: Arc<AtomicUsize>,
    default_name: Arc<Mutex<Option<String>>>,
    probe_error: Arc<Mutex<Option<String>>>,
}

impl FakeHandles {
    fn latest(&self) -> std::sync::MutexGuard<'_, VecDeque<FakeHandle>> {
        self.queue.lock().unwrap()
    }
    fn attached_names(events: &Captured) -> Vec<String> {
        events
            .events
            .lock()
            .unwrap()
            .iter()
            .filter_map(|e| match e {
                SystemAudioEvent::Attached { device_name, .. } => Some(device_name.clone()),
                _ => None,
            })
            .collect()
    }
}

#[test]
fn reattach_delay_doubles_then_caps() {
    assert_eq!(reattach_delay(0), Duration::from_millis(100));
    assert_eq!(reattach_delay(1), Duration::from_millis(200));
    assert_eq!(reattach_delay(2), Duration::from_millis(400));
    assert_eq!(reattach_delay(3), Duration::from_millis(800));
    assert_eq!(reattach_delay(4), Duration::from_millis(1600));
    assert_eq!(reattach_delay(9), Duration::from_millis(1600));
}

#[test]
fn assess_stream_detaches_on_failure_or_default_change() {
    let named = |name: &str| Ok(Some(name.to_string()));
    assert_eq!(
        assess_stream(Some("invalidated".into()), "Speakers", named("Speakers")),
        Some(DetachReason::StreamError("invalidated".into()))
    );
    assert_eq!(
        assess_stream(None, "Speakers", named("Headset")),
        Some(DetachReason::DefaultDeviceChanged)
    );
    assert_eq!(
        assess_stream(None, "Speakers", Ok(None)),
        Some(DetachReason::DefaultDeviceChanged)
    );
    assert_eq!(assess_stream(None, "Speakers", named("Speakers")), None);
    // A probe failure is not evidence of a swap — keep the stream; a real
    // failure still detaches ahead of the broken probe.
    assert_eq!(
        assess_stream(None, "Speakers", Err("name() failed".into())),
        None,
        "a transient probe error must not detach a healthy stream"
    );
    assert_eq!(
        assess_stream(Some("invalidated".into()), "Speakers", Err("boom".into())),
        Some(DetachReason::StreamError("invalidated".into()))
    );
}

#[test]
fn frames_flow_through_the_pipeline() {
    let backend = FakeBackend::new("Speakers");
    let (capture, captured, handles) = start(backend);

    assert!(wait_for(|| !handles.latest().is_empty()));
    let mut guard = handles.latest();
    let handle = guard.front_mut().expect("one opened stream");
    for _ in 0..4 {
        feed(&mut handle.producer, &[0.5f32; 480]);
    }
    drop(guard);

    // 16 kHz in == out: the resampler is pass-through, so frames arrive intact.
    assert!(wait_for(|| captured.frames.lock().unwrap().len() >= 4));
    assert!(captured
        .frames
        .lock()
        .unwrap()
        .iter()
        .all(|f| f.len() == 480));
    assert_eq!(
        FakeHandles::attached_names(&captured),
        vec!["Speakers".to_string()]
    );

    capture.stop();
}

#[test]
fn default_device_swap_detaches_and_reattaches() {
    let backend = FakeBackend::new("Speakers");
    let (capture, captured, handles) = start(backend);
    assert!(wait_for(|| !handles.latest().is_empty()));

    handles
        .default_name
        .lock()
        .unwrap()
        .replace("Headset".to_string());

    assert!(wait_for(|| {
        let events = captured.events.lock().unwrap();
        events.contains(&SystemAudioEvent::Detached {
            reason: DetachReason::DefaultDeviceChanged,
        }) && events.iter().any(|e| {
            matches!(
                e,
                SystemAudioEvent::Attached { device_name, after_gap }
                    if device_name == "Headset" && after_gap.is_some()
            )
        })
    }));

    // Audio keeps flowing from the new endpoint's stream.
    assert!(wait_for(|| handles.latest().len() >= 2));
    let mut guard = handles.latest();
    feed(
        &mut guard.get_mut(1).expect("second stream").producer,
        &[0.25; 480],
    );
    drop(guard);
    assert!(wait_for(|| !captured.frames.lock().unwrap().is_empty()));

    capture.stop();
}

#[test]
fn stream_failure_detaches_then_reattaches() {
    let backend = FakeBackend::new("Speakers");
    let (capture, captured, handles) = start(backend);
    assert!(wait_for(|| !handles.latest().is_empty()));

    *handles
        .latest()
        .front()
        .expect("stream")
        .failure
        .lock()
        .unwrap() = Some("device invalidated".to_string());

    assert!(wait_for(|| {
        captured
            .events
            .lock()
            .unwrap()
            .contains(&SystemAudioEvent::Detached {
                reason: DetachReason::StreamError("device invalidated".into()),
            })
    }));
    assert!(wait_for(|| handles.latest().len() >= 2));

    capture.stop();
}

#[test]
fn failed_open_retries_and_reports_the_gap() {
    let backend = FakeBackend::new("Speakers");
    *backend.open_error.lock().unwrap() = Some("endpoint busy".to_string());
    let (capture, captured, handles) = start(backend);

    assert!(wait_for(|| {
        captured
            .events
            .lock()
            .unwrap()
            .contains(&SystemAudioEvent::Detached {
                reason: DetachReason::OpenFailed("endpoint busy".into()),
            })
    }));
    // The one-shot error is consumed; the retry attaches and reports a gap.
    assert!(wait_for(|| {
        captured.events.lock().unwrap().iter().any(|e| {
            matches!(
                e,
                SystemAudioEvent::Attached {
                    after_gap: Some(_),
                    ..
                }
            )
        })
    }));
    assert!(handles.opens.load(Ordering::Relaxed) >= 2);

    capture.stop();
}

#[test]
fn stop_during_an_outage_exits_promptly() {
    let backend = FakeBackend::new("Speakers");
    *backend.default_name.lock().unwrap() = None; // no endpoint: open keeps failing
    let (capture, _captured, _handles) = start(backend);

    let stopped = Instant::now();
    capture.stop();
    assert!(stopped.elapsed() < Duration::from_secs(2));
}

/// Marker value for feed block `i` — small, non-zero and unique per block so
/// a gap, duplication or reordering is visible in the delivered stream.
fn marker(i: usize) -> f32 {
    (i % 900 + 1) as f32 * 1e-4
}

/// Sustained input at the *production* assessment cadence: audio arrives
/// faster than the old one-chunk-per-poll drain could consume, so any drain
/// deficit overflows the ring. Feeding goes through the same
/// `write_input_to_ring` the cpal callback uses, so whatever cannot fit is
/// counted in the transport's overrun counter exactly like hardware.
#[test]
fn sustained_input_is_drained_without_loss() {
    let backend = FakeBackend::new("Speakers");
    let (capture, captured, handles) = start_with_poll(backend, POLL_INTERVAL);
    assert!(wait_for(|| !handles.latest().is_empty()));

    let transport = Arc::new(CaptureTransportState::default());
    const CHUNK: usize = 160; // 10 ms at 16 kHz
    const CHUNKS: usize = 384; // ~3.84 s of continuous audio — more than the 2 s ring
    {
        let mut guard = handles.latest();
        let handle = guard.front_mut().expect("one opened stream");
        for i in 0..CHUNKS {
            let block = vec![marker(i); CHUNK];
            AudioRecorder::write_input_to_ring(&block, 1, None, &mut handle.producer, &transport);
            // Arrive ~6.7× faster than real time; still far below what the
            // drain must handle, so a correct consumer loses nothing.
            std::thread::sleep(Duration::from_micros(1500));
        }
    }

    capture.stop();

    assert_eq!(
        transport.overrun_samples.load(Ordering::Relaxed),
        0,
        "the drain must keep up with sustained input"
    );

    let delivered: Vec<f32> = captured
        .frames
        .lock()
        .unwrap()
        .iter()
        .flatten()
        .copied()
        .collect();
    assert_eq!(
        delivered.len(),
        CHUNK * CHUNKS,
        "every fed sample reaches the consumer"
    );
    for (pos, sample) in delivered.iter().enumerate() {
        assert_eq!(
            *sample,
            marker(pos / CHUNK),
            "marker mismatch at delivered sample {pos}"
        );
    }
}

/// The same sustained load on a 48 kHz endpoint: real resampling must keep
/// the input duration, not drop backlog the resampler could not absorb.
#[test]
fn sustained_48khz_input_is_resampled_without_loss() {
    let mut backend = FakeBackend::new("Speakers");
    backend.sample_rate = 48_000;
    let (capture, captured, handles) = start_with_poll(backend, POLL_INTERVAL);
    assert!(wait_for(|| !handles.latest().is_empty()));

    let transport = Arc::new(CaptureTransportState::default());
    const CHUNK: usize = 480; // 10 ms at 48 kHz
    const CHUNKS: usize = 300; // 3 s of audio > the 2 s ring
    {
        let mut guard = handles.latest();
        let handle = guard.front_mut().expect("one opened stream");
        for _ in 0..CHUNKS {
            AudioRecorder::write_input_to_ring(
                &[0.25f32; CHUNK],
                1,
                None,
                &mut handle.producer,
                &transport,
            );
            std::thread::sleep(Duration::from_micros(1500));
        }
    }

    capture.stop();

    assert_eq!(
        transport.overrun_samples.load(Ordering::Relaxed),
        0,
        "the drain must keep up with sustained 48 kHz input"
    );

    let delivered: usize = captured
        .frames
        .lock()
        .unwrap()
        .iter()
        .map(|f| f.len())
        .sum();
    let expected = CHUNK * CHUNKS / 3; // 48 kHz → 16 kHz
    let tolerance = expected / 50; // 2 %: filter edges and the finish tail
    assert!(
        delivered.abs_diff(expected) <= tolerance,
        "resampled duration {delivered} vs expected ~{expected}"
    );
}

/// A stream that reports drops — however the backend counts them — surfaces
/// an `Overrun` event on every counter growth so the session accumulates
/// the deltas; reporting only the first would undercount a leaky attach.
#[test]
fn ring_overrun_reports_every_growth() {
    let backend = FakeBackend::new("Speakers");
    let (capture, captured, handles) = start(backend);
    assert!(wait_for(|| !handles.latest().is_empty()));

    handles
        .latest()
        .front()
        .expect("stream")
        .dropped
        .store(4_800, Ordering::Relaxed);

    assert!(wait_for(|| {
        captured.events.lock().unwrap().iter().any(|e| {
            matches!(
                e,
                SystemAudioEvent::Overrun { dropped_samples } if *dropped_samples == 4_800
            )
        })
    }));

    // The counter keeps growing — a second event must follow.
    handles
        .latest()
        .front()
        .expect("stream")
        .dropped
        .store(9_600, Ordering::Relaxed);

    assert!(wait_for(|| {
        captured.events.lock().unwrap().iter().any(|e| {
            matches!(
                e,
                SystemAudioEvent::Overrun { dropped_samples } if *dropped_samples == 9_600
            )
        })
    }));

    capture.stop();
}

/// A detach must deliver the ring's queued audio before reporting the gap —
/// samples that already arrived are not lost just because the stream died.
/// Frames and lifecycle events share one ordered log (both callbacks run on
/// the worker thread), so the assertion proves ordering, not just totals.
#[test]
fn detach_drains_queued_samples_before_reporting() {
    enum Log {
        Frame(usize),
        Event(SystemAudioEvent),
    }
    let log = Arc::new(Mutex::new(Vec::new()));
    let frame_log = Arc::clone(&log);
    let event_log = Arc::clone(&log);
    let backend = FakeBackend::new("Speakers");
    let handles = FakeHandles {
        queue: backend.take_handle(),
        opens: Arc::clone(&backend.opens),
        default_name: Arc::clone(&backend.default_name),
        probe_error: Arc::clone(&backend.probe_error),
    };
    let capture = SystemAudioCapture::start_with_backend(
        Box::new(backend),
        Duration::from_millis(5),
        Arc::new(move |frame| frame_log.lock().unwrap().push(Log::Frame(frame.len()))),
        Arc::new(move |event| event_log.lock().unwrap().push(Log::Event(event))),
    )
    .expect("capture starts");
    assert!(wait_for(|| !handles.latest().is_empty()));

    // Push 640 samples with markers, then fail the stream. The normal
    // drain may pick them up first — the resampler's sub-frame remainder
    // can only be flushed by the detach path's `finish` — but whichever
    // internal path delivers them, the contract under test is that every
    // fed sample is logged strictly before `Detached`.
    const FED: usize = 640;
    {
        let mut guard = handles.latest();
        let handle = guard.front_mut().expect("one opened stream");
        feed(
            &mut handle.producer,
            &(0..FED).map(marker).collect::<Vec<_>>(),
        );
        *handle.failure.lock().unwrap() = Some("endpoint gone".to_string());
    }

    assert!(wait_for(|| {
        log.lock().unwrap().iter().any(|e| {
            matches!(
                e,
                Log::Event(SystemAudioEvent::Detached {
                    reason: DetachReason::StreamError(_)
                })
            )
        })
    }));

    // Detach may keep reattaching — freeze the sequence at the first
    // `Detached` and compare against what was logged before it.
    let stop = capture;
    let entries = log.lock().unwrap();
    let detach_at = entries
        .iter()
        .position(|e| matches!(e, Log::Event(SystemAudioEvent::Detached { .. })))
        .expect("detach logged");
    let delivered_before_detach: usize = entries[..detach_at]
        .iter()
        .filter_map(|e| match e {
            Log::Frame(n) => Some(*n),
            _ => None,
        })
        .sum();
    // The resampler tail flush pads the final sub-frame remainder, so the
    // delivered count can exceed FED — the contract is that nothing fed is
    // lost and every delivered frame precedes the `Detached` event.
    assert!(
        delivered_before_detach >= FED,
        "queued samples delivered before `Detached`: {delivered_before_detach}/{FED}"
    );
    // `stop()` joins the worker, whose callbacks lock `log` — release the
    // guard first or a late frame/reattach event deadlocks the join.
    drop(entries);

    stop.stop();
}

/// While the default-endpoint probe fails, even a changed name must not
/// detach the stream — once the probe recovers the real swap is honored.
#[test]
fn probe_failures_do_not_reattach() {
    let backend = FakeBackend::new("Speakers");
    let (capture, captured, handles) = start(backend);
    assert!(wait_for(|| !handles.latest().is_empty()));

    *handles.probe_error.lock().unwrap() = Some("transient name() failure".to_string());
    handles
        .default_name
        .lock()
        .unwrap()
        .replace("Headset".to_string());

    // Several 5 ms poll ticks pass — none may detach.
    std::thread::sleep(Duration::from_millis(200));
    assert!(
        captured
            .events
            .lock()
            .unwrap()
            .iter()
            .all(|e| !matches!(e, SystemAudioEvent::Detached { .. })),
        "a broken probe must not flap the capture"
    );

    // Probe recovery observes the real swap and reattaches once.
    *handles.probe_error.lock().unwrap() = None;
    assert!(wait_for(|| {
        FakeHandles::attached_names(&captured)
            .iter()
            .any(|name| name == "Headset")
    }));

    capture.stop();
}

/// Drops counted between assessments must still surface when the stream
/// detaches — the loss happened inside this attach, so this attach reports
/// it before the `Detached` event.
#[test]
fn detach_reports_the_final_overrun() {
    let backend = FakeBackend::new("Speakers");
    let (capture, captured, handles) = start_with_poll(backend, Duration::from_millis(250));
    assert!(wait_for(|| !handles.latest().is_empty()));

    {
        let mut guard = handles.latest();
        let handle = guard.front_mut().expect("one opened stream");
        handle.dropped.store(1_600, Ordering::Relaxed);
        *handle.failure.lock().unwrap() = Some("endpoint gone".to_string());
    }

    // The 250 ms cadence means no periodic assessment ran between the
    // counter bump and the detach — only the exit path could emit it.
    assert!(wait_for(|| {
        let events = captured.events.lock().unwrap();
        let overrun_at = events.iter().position(|e| {
            matches!(
                e,
                SystemAudioEvent::Overrun { dropped_samples } if *dropped_samples == 1_600
            )
        });
        let detach_at = events
            .iter()
            .position(|e| matches!(e, SystemAudioEvent::Detached { .. }));
        match (overrun_at, detach_at) {
            (Some(o), Some(d)) => o < d,
            _ => false,
        }
    }));

    capture.stop();
}

/// A stop must not lose the last overrun window either: the summary the
/// session logs at stop is the only record of samples dropped after the
/// final periodic check.
#[test]
fn stop_reports_the_final_overrun() {
    let backend = FakeBackend::new("Speakers");
    let (capture, captured, handles) = start_with_poll(backend, Duration::from_secs(60));
    assert!(wait_for(|| !handles.latest().is_empty()));

    // With a 60 s assessment cadence no periodic overrun check runs — the
    // only possible report is the stop path's final reading.
    handles
        .latest()
        .front()
        .expect("stream")
        .dropped
        .store(3_200, Ordering::Relaxed);
    capture.stop();

    assert!(captured.events.lock().unwrap().iter().any(|e| {
        matches!(
            e,
            SystemAudioEvent::Overrun { dropped_samples } if *dropped_samples == 3_200
        )
    }));
}

/// Real-machine probe: `TRANSCREVE_PROBE_SCK=1 cargo test sck_probe` opens a
/// live ScreenCaptureKit tap for a few seconds and reports how much audio it
/// captured. Needs Screen Recording permission granted to the test binary —
/// without it `open` fails with the permission message, which is what this
/// probe exists to show.
#[cfg(target_os = "macos")]
#[test]
fn sck_probe_reports_capture() {
    if std::env::var("TRANSCREVE_PROBE_SCK").is_err() {
        return;
    }
    let backend = super::mac::SckLoopbackBackend;
    match backend.open() {
        Err(message) => println!("PROBE-SCK: open failed: {message}"),
        Ok(opened) => {
            std::thread::sleep(Duration::from_secs(3));
            println!(
                "PROBE-SCK: attached on {:?}, rate={}, queued={} samples, dropped={}",
                opened.stream.device_name(),
                opened.stream.sample_rate(),
                opened.samples.slots(),
                opened.stream.dropped_samples()
            );
        }
    }
}
