//! First-run microphone check (F010 setup screen): opens the selected input
//! device on a dedicated thread and streams a normalized level to the hub so
//! the user can see the mic is picking up sound. Deliberately independent from
//! `AudioRecordingManager` — no VAD, no recording, and no samples ever leave
//! this module; only one peak value per ~33 ms is emitted.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, Sample, SizedSample};
use log::{debug, warn};
use serde::Serialize;
use specta::Type;
use tauri::{AppHandle, Emitter};

use crate::audio_toolkit::audio::list_input_devices;
use crate::window_labels;

/// `{ level: f32 }` in 0..=1, ~30 Hz while a test runs.
pub const LEVEL_EVENT: &str = "onboarding://mic-level";
/// Emitted once when a test ends (stop, auto-timeout or device error).
pub const STOPPED_EVENT: &str = "onboarding://mic-test-stopped";

const EMIT_INTERVAL: Duration = Duration::from_millis(33);
/// A forgotten test never keeps the mic open for long.
const MAX_DURATION: Duration = Duration::from_secs(30);
/// Floor of the dB scale used for the meter.
const FLOOR_DB: f32 = -60.0;

struct ActiveTest {
    stop: Sender<()>,
    thread: JoinHandle<()>,
}

static ACTIVE: Mutex<Option<ActiveTest>> = Mutex::new(None);

#[derive(Clone, Serialize, Type)]
pub struct MicTestLevel {
    pub level: f32,
}

#[derive(Clone, Serialize, Type)]
pub struct MicTestStarted {
    /// Display name of the device being tested.
    pub device_name: String,
}

/// Peak amplitude (0..=1 linear) mapped onto a 0..=1 meter over
/// `FLOOR_DB..0 dBFS`, so normal speech lands mid-scale instead of hugging 0.
pub fn normalized_level(peak: f32) -> f32 {
    if !peak.is_finite() || peak <= 0.0 {
        return 0.0;
    }
    let db = 20.0 * peak.min(1.0).log10();
    ((db - FLOOR_DB) / -FLOOR_DB).clamp(0.0, 1.0)
}

fn resolve_device(selected: Option<&str>) -> Result<(cpal::Device, String), String> {
    if let Some(name) = selected {
        let devices = list_input_devices().map_err(|e| e.to_string())?;
        if let Some(info) = devices.into_iter().find(|d| d.name == name) {
            return Ok((info.device, info.name));
        }
        warn!("mic test: selected microphone not found, falling back to default");
    }
    let device = crate::audio_toolkit::get_cpal_host()
        .default_input_device()
        .ok_or_else(|| "no input device available".to_string())?;
    let name = device.name().unwrap_or_else(|_| "Default".into());
    Ok((device, name))
}

fn build_stream<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    peak: Arc<AtomicU32>,
) -> Result<cpal::Stream, cpal::BuildStreamError>
where
    T: Sample + SizedSample,
    f32: FromSample<T>,
{
    device.build_input_stream(
        config,
        move |data: &[T], _: &cpal::InputCallbackInfo| {
            // Real-time callback: lock- and allocation-free.
            let local = data
                .iter()
                .map(|&s| f32::from_sample(s).abs())
                .fold(0.0_f32, f32::max);
            if local > f32::from_bits(peak.load(Ordering::Relaxed)) {
                peak.store(local.to_bits(), Ordering::Relaxed);
            }
        },
        |err| debug!("mic test stream error: {err}"),
        None,
    )
}

fn open_stream(device: &cpal::Device, peak: Arc<AtomicU32>) -> Result<cpal::Stream, String> {
    let supported = device.default_input_config().map_err(|e| e.to_string())?;
    let config: cpal::StreamConfig = supported.clone().into();
    let stream = match supported.sample_format() {
        cpal::SampleFormat::F32 => build_stream::<f32>(device, &config, peak),
        cpal::SampleFormat::I16 => build_stream::<i16>(device, &config, peak),
        cpal::SampleFormat::I32 => build_stream::<i32>(device, &config, peak),
        cpal::SampleFormat::U16 => build_stream::<u16>(device, &config, peak),
        cpal::SampleFormat::U8 => build_stream::<u8>(device, &config, peak),
        cpal::SampleFormat::I8 => build_stream::<i8>(device, &config, peak),
        other => return Err(format!("unsupported sample format {other:?}")),
    }
    .map_err(|e| e.to_string())?;
    stream.play().map_err(|e| e.to_string())?;
    Ok(stream)
}

fn run(
    app: AppHandle,
    selected: Option<String>,
    stop: mpsc::Receiver<()>,
    ready: mpsc::Sender<Result<String, String>>,
) {
    let peak = Arc::new(AtomicU32::new(0));
    let opened = resolve_device(selected.as_deref())
        .and_then(|(device, name)| open_stream(&device, peak.clone()).map(|s| (s, name)));
    let stream = match opened {
        Ok((stream, name)) => {
            let _ = ready.send(Ok(name));
            stream
        }
        Err(e) => {
            let _ = ready.send(Err(e));
            return;
        }
    };

    let started = Instant::now();
    loop {
        match stop.recv_timeout(EMIT_INTERVAL) {
            Ok(()) | Err(RecvTimeoutError::Disconnected) => break,
            Err(RecvTimeoutError::Timeout) => {}
        }
        if started.elapsed() >= MAX_DURATION {
            break;
        }
        let level = normalized_level(f32::from_bits(peak.swap(0, Ordering::Relaxed)));
        let _ = app.emit_to(window_labels::HUB, LEVEL_EVENT, MicTestLevel { level });
    }
    drop(stream);
    let _ = app.emit_to(window_labels::HUB, STOPPED_EVENT, ());
}

/// Stop the running test, if any, and wait for its thread to release the
/// device. Blocking (≤ one emit interval plus the stream teardown).
pub fn stop() {
    let active = ACTIVE.lock().unwrap_or_else(|e| e.into_inner()).take();
    if let Some(active) = active {
        let _ = active.stop.send(());
        if active.thread.join().is_err() {
            warn!("mic test thread panicked");
        }
    }
}

/// Start a test on `selected` (the persisted microphone name; `None` = system
/// default), replacing a running one. Blocking until the device is open.
pub fn start(app: &AppHandle, selected: Option<String>) -> Result<MicTestStarted, String> {
    stop();
    let (stop_tx, stop_rx) = mpsc::channel();
    let (ready_tx, ready_rx) = mpsc::channel();
    let thread_app = app.clone();
    let thread = std::thread::Builder::new()
        .name("mic-test".into())
        .spawn(move || run(thread_app, selected, stop_rx, ready_tx))
        .map_err(|e| e.to_string())?;

    match ready_rx.recv() {
        Ok(Ok(device_name)) => {
            *ACTIVE.lock().unwrap_or_else(|e| e.into_inner()) = Some(ActiveTest {
                stop: stop_tx,
                thread,
            });
            Ok(MicTestStarted { device_name })
        }
        Ok(Err(e)) => {
            let _ = thread.join();
            Err(e)
        }
        Err(_) => {
            let _ = thread.join();
            Err("mic test thread exited before opening the device".into())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::normalized_level;

    #[test]
    fn silence_and_invalid_input_read_zero() {
        assert_eq!(normalized_level(0.0), 0.0);
        assert_eq!(normalized_level(-0.5), 0.0);
        assert_eq!(normalized_level(f32::NAN), 0.0);
        assert_eq!(normalized_level(0.0001), 0.0); // -80 dBFS, below the floor
    }

    #[test]
    fn full_scale_reads_one_and_clips_above() {
        assert_eq!(normalized_level(1.0), 1.0);
        assert_eq!(normalized_level(4.0), 1.0);
    }

    #[test]
    fn scale_is_logarithmic() {
        // 0.1 linear = -20 dBFS → two thirds of a -60..0 scale.
        assert!((normalized_level(0.1) - 2.0 / 3.0).abs() < 1e-4);
        // 0.001 linear = -60 dBFS → the floor.
        assert!(normalized_level(0.001).abs() < 1e-4);
    }
}
