//! Scripted-fake tests for the loopback worker: the real device is unreachable
//! from tests, so [`FakeBackend`] scripts endpoint names, open failures and
//! mid-stream errors while the genuine attach/drain/resample/detach logic runs.

use super::*;
use rtrb::{Producer, RingBuffer};
use std::{
    collections::VecDeque,
    sync::atomic::{AtomicUsize, Ordering},
    time::Duration,
};

/// Everything a test needs to drive an opened fake stream: push samples into
/// its ring and flip its failure flag.
struct FakeHandle {
    producer: Producer<f32>,
    failure: Arc<Mutex<Option<String>>>,
}

struct FakeStream {
    device_name: String,
    sample_rate: u32,
    failure: Arc<Mutex<Option<String>>>,
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
}

struct FakeBackend {
    default_name: Arc<Mutex<Option<String>>>,
    /// One-shot open failure: consumed by the next `open` call.
    open_error: Arc<Mutex<Option<String>>>,
    sample_rate: u32,
    opens: Arc<AtomicUsize>,
    handles: Arc<Mutex<VecDeque<FakeHandle>>>,
}

impl FakeBackend {
    fn new(default_name: &str) -> Self {
        Self {
            default_name: Arc::new(Mutex::new(Some(default_name.to_string()))),
            open_error: Arc::new(Mutex::new(None)),
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
    fn default_device_name(&self) -> Option<String> {
        self.default_name.lock().unwrap().clone()
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
        self.handles.lock().unwrap().push_back(FakeHandle {
            producer,
            failure: Arc::clone(&failure),
        });
        Ok(OpenedLoopback {
            stream: Box::new(FakeStream {
                device_name,
                sample_rate: self.sample_rate,
                failure,
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
    };
    let capture = SystemAudioCapture::start_with_backend(
        Box::new(backend),
        Duration::from_millis(5),
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
    assert_eq!(
        assess_stream(Some("invalidated".into()), "Speakers", Some("Speakers")),
        Some(DetachReason::StreamError("invalidated".into()))
    );
    assert_eq!(
        assess_stream(None, "Speakers", Some("Headset")),
        Some(DetachReason::DefaultDeviceChanged)
    );
    assert_eq!(
        assess_stream(None, "Speakers", None),
        Some(DetachReason::DefaultDeviceChanged)
    );
    assert_eq!(assess_stream(None, "Speakers", Some("Speakers")), None);
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
