//! WAV file audio source for E2E test builds (T-009; F002 technical notes).
//!
//! This module only exists under the `audio-fixture` cargo feature, which
//! release builds never enable. When the `TRANSCREVE_AUDIO_FIXTURE` env var
//! points at a WAV file, [`AudioRecorder::open`](super::AudioRecorder::open)
//! feeds that file into the capture ring at real-time pace instead of opening
//! a cpal input stream, so the whole downstream pipeline — resampler, VAD,
//! frame subscribers, recording buffer — runs unchanged end to end.
//!
//! Playback is gated on the session signal the consumer publishes on
//! `Cmd::Start`/`Cmd::Stop`: while no dictation session is recording the feeder
//! emits silence, and each new session rewinds the file to the beginning. The
//! result is the deterministic "press shortcut → utterance plays → release"
//! flow the `windows-desktop-e2e` harness drives.

use std::{
    io::Error,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};

use rtrb::{Consumer, Producer, RingBuffer};

use super::recorder::{AudioRecorder, CaptureTransportState};

/// Environment variable selecting the WAV fixture. Honored only in builds
/// compiled with `--features audio-fixture`.
pub const FIXTURE_ENV_VAR: &str = "TRANSCREVE_AUDIO_FIXTURE";

/// Seconds of ring capacity — mirrors the microphone path so drains behave the
/// same way on both sources.
const RING_SECONDS: usize = 2;

/// Feeder push interval, matching the consumer's poll cadence.
const PUSH_INTERVAL: Duration = Duration::from_millis(10);

/// Resolve the configured WAV fixture path, if any. An empty value counts as
/// unset; relative paths resolve against the process working directory.
pub fn fixture_path_from_env() -> Option<PathBuf> {
    std::env::var_os(FIXTURE_ENV_VAR)
        .map(PathBuf::from)
        .filter(|path| !path.as_os_str().is_empty())
}

/// Decoded fixture audio: mono f32 at the file's native sample rate.
#[derive(Debug)]
pub struct FixtureAudio {
    pub sample_rate: u32,
    pub samples: Vec<f32>,
}

/// Load a WAV file as mono f32 samples, averaging extra channels. Any sample
/// rate is accepted — the shared `FrameResampler` converts to 16 kHz exactly
/// like it does for a live device. Int files read through `i32` and are
/// normalized by the declared bit depth (hound returns the file's stored
/// integer range, unscaled); float files read through `f32`.
pub fn load_wav(path: &Path) -> Result<FixtureAudio, Error> {
    let reader = hound::WavReader::open(path)
        .map_err(|e| Error::new(std::io::ErrorKind::InvalidData, e.to_string()))?;
    let spec = reader.spec();
    let channels = spec.channels as usize;
    if channels == 0 || spec.sample_rate == 0 {
        return Err(Error::new(
            std::io::ErrorKind::InvalidData,
            "WAV file declares zero channels or a zero sample rate",
        ));
    }

    // Full-scale divisor for the file's integer range: 2^(bits-1), so i16
    // full scale maps to ~1.0.
    let int_scale = 2f64.powi(spec.bits_per_sample.saturating_sub(1) as i32) as f32;
    let interleaved: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Int => reader
            .into_samples::<i32>()
            .map(|sample| sample.map(|v| v as f32 / int_scale))
            .collect::<Result<Vec<f32>, _>>()
            .map_err(|e| Error::new(std::io::ErrorKind::InvalidData, e.to_string()))?,
        hound::SampleFormat::Float => reader
            .into_samples::<f32>()
            .collect::<Result<Vec<f32>, _>>()
            .map_err(|e| Error::new(std::io::ErrorKind::InvalidData, e.to_string()))?,
    };

    let samples = interleaved
        .chunks_exact(channels)
        .map(|frame| frame.iter().sum::<f32>() / channels as f32)
        .collect();

    Ok(FixtureAudio {
        sample_rate: spec.sample_rate,
        samples,
    })
}

/// Real-time feeder that writes fixture audio into the capture ring. Dropping
/// it stops the feeder thread, mirroring how dropping a cpal stream ends a
/// microphone callback.
pub struct FixtureFeeder {
    shutdown: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl FixtureFeeder {
    /// Load `path` and start feeding the ring in real time. Returns the feeder
    /// guard, the ring's consumer end, and the file's sample rate (which the
    /// caller uses as `in_sample_rate` for the resampler).
    pub fn start(
        path: &Path,
        transport: Arc<CaptureTransportState>,
    ) -> Result<(Self, Consumer<f32>, u32), Error> {
        Self::start_with_pace(path, transport, true)
    }

    /// `realtime: false` pushes as fast as the ring accepts — used by unit
    /// tests that cannot wait for wall-clock playback.
    pub(crate) fn start_with_pace(
        path: &Path,
        transport: Arc<CaptureTransportState>,
        realtime: bool,
    ) -> Result<(Self, Consumer<f32>, u32), Error> {
        let audio = load_wav(path)?;
        let sample_rate = audio.sample_rate;
        let ring_capacity = sample_rate as usize * RING_SECONDS;
        let (producer, consumer) = RingBuffer::new(ring_capacity);
        let shutdown = Arc::new(AtomicBool::new(false));
        let handle = {
            let shutdown = Arc::clone(&shutdown);
            std::thread::Builder::new()
                .name("wav-fixture-feeder".into())
                .spawn(move || run_feeder(producer, audio, transport, shutdown, realtime))
                .map_err(|e| Error::other(format!("Failed to spawn fixture feeder: {e}")))?
        };
        Ok((
            Self {
                shutdown,
                handle: Some(handle),
            },
            consumer,
            sample_rate,
        ))
    }
}

impl Drop for FixtureFeeder {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Release);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

/// Feeder body. Every iteration pushes one ~10 ms block through the same
/// ring-write path the cpal callback uses (so pause handshakes and overrun
/// accounting behave identically); the block is fixture audio while a session
/// is recording and silence otherwise, with playback restarting from the top
/// each time a session begins.
fn run_feeder(
    mut producer: Producer<f32>,
    audio: FixtureAudio,
    transport: Arc<CaptureTransportState>,
    shutdown: Arc<AtomicBool>,
    realtime: bool,
) {
    let chunk = (audio.sample_rate as usize * PUSH_INTERVAL.as_millis() as usize / 1000).max(1);
    let silence = vec![0.0f32; chunk];
    let mut position = 0usize;
    let mut session_was_active = false;
    let started = Instant::now();
    // Samples written so far, for wall-clock pacing.
    let mut written = 0usize;

    while !shutdown.load(Ordering::Acquire) {
        let session_active = transport.session_active.load(Ordering::Acquire);
        if session_active && !session_was_active {
            position = 0;
        }
        session_was_active = session_active;

        let clip_active = session_active && position < audio.samples.len();
        let block: &[f32] = if clip_active {
            let end = (position + chunk).min(audio.samples.len());
            &audio.samples[position..end]
        } else {
            &silence
        };
        let block_len = block.len();

        // Backpressure instead of overrunning: unlike a live mic callback the
        // fixture can wait, and dropping blocks would silently truncate the
        // scripted utterance. `shutdown` still lets teardown break the wait.
        while producer.slots() < block_len {
            if shutdown.load(Ordering::Acquire) {
                return;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        AudioRecorder::write_input_to_ring(block, 1, None, &mut producer, &transport);
        if clip_active {
            position += block_len;
        }
        written += block_len;

        if realtime {
            let target =
                started + Duration::from_secs_f64(written as f64 / audio.sample_rate as f64);
            if let Some(delay) = target.checked_duration_since(Instant::now()) {
                std::thread::sleep(delay);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_wav(path: &Path, spec: hound::WavSpec, samples: &[i32]) {
        // The test WAVs are always 16-bit PCM; `samples` are raw i16 values
        // widened to i32 for a uniform call site.
        assert_eq!(spec.bits_per_sample, 16);
        assert_eq!(spec.sample_format, hound::SampleFormat::Int);
        let mut writer = hound::WavWriter::create(path, spec).expect("create wav");
        for &sample in samples {
            writer.write_sample(sample as i16).expect("write sample");
        }
        writer.finalize().expect("finalize wav");
    }

    fn spec(channels: u16, sample_rate: u32) -> hound::WavSpec {
        hound::WavSpec {
            channels,
            sample_rate,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        }
    }

    #[test]
    fn fixture_path_from_env_round_trips_and_ignores_empty() {
        // SAFETY: tests run in the same process; serialize via a single lock.
        static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _guard = ENV_LOCK.lock().unwrap();

        std::env::remove_var(FIXTURE_ENV_VAR);
        assert!(fixture_path_from_env().is_none());

        std::env::set_var(FIXTURE_ENV_VAR, "");
        assert!(fixture_path_from_env().is_none());

        std::env::set_var(FIXTURE_ENV_VAR, "e2e/fixtures/pt-br/fala_curta.wav");
        assert_eq!(
            fixture_path_from_env(),
            Some(PathBuf::from("e2e/fixtures/pt-br/fala_curta.wav"))
        );

        std::env::remove_var(FIXTURE_ENV_VAR);
    }

    #[test]
    fn load_wav_reads_16khz_mono_as_normalized_f32() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("mono.wav");
        write_wav(&path, spec(1, 16_000), &[0, 16384, -16384, i16::MAX as i32]);

        let audio = load_wav(&path).expect("load wav");

        assert_eq!(audio.sample_rate, 16_000);
        assert_eq!(audio.samples.len(), 4);
        assert_eq!(audio.samples[0], 0.0);
        assert!((audio.samples[1] - 0.5).abs() < 0.001);
        assert!((audio.samples[2] + 0.5).abs() < 0.001);
    }

    #[test]
    fn load_wav_downmixes_stereo_to_mono() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("stereo.wav");
        // Frames [0.5, 1.0] → 0.75 and [-0.5, 0.5] → 0.0 (in i16 terms).
        write_wav(&path, spec(2, 48_000), &[16_384, 32_767, -16_384, 16_384]);

        let audio = load_wav(&path).expect("load wav");

        assert_eq!(audio.sample_rate, 48_000);
        assert_eq!(audio.samples.len(), 2);
        assert!((audio.samples[0] - 0.75).abs() < 0.001);
        assert!(audio.samples[1].abs() < 0.001);
    }

    #[test]
    fn load_wav_rejects_missing_file() {
        let err = load_wav(Path::new("does/not/exist.wav")).expect_err("must fail");
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
    }

    /// Drive the transport's session flag the way `run_consumer` does.
    fn set_session(transport: &Arc<CaptureTransportState>, active: bool) {
        transport.session_active.store(active, Ordering::Release);
    }

    #[test]
    fn feeder_plays_file_during_session_and_silence_when_idle() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("clip.wav");
        // 0.25 s of 0.5 amplitude at 16 kHz; ring holds 2 s, so the whole clip
        // fits even with the consumer not draining yet.
        let clip: Vec<i32> = vec![i16::MAX as i32 / 2; 4_000];
        write_wav(&path, spec(1, 16_000), &clip);

        let transport = Arc::new(CaptureTransportState::default());
        // Non-realtime pace: push as fast as the ring accepts.
        let (feeder, mut consumer, rate) =
            FixtureFeeder::start_with_pace(&path, Arc::clone(&transport), false)
                .expect("start feeder");

        assert_eq!(rate, 16_000);

        // Idle: the feeder emits silence.
        std::thread::sleep(Duration::from_millis(50));
        let idle = drain_all(&mut consumer);
        assert!(!idle.is_empty());
        assert!(idle.iter().all(|&s| s == 0.0), "idle must be silent");

        // Session start rewinds and plays the clip.
        set_session(&transport, true);
        let deadline = Instant::now() + Duration::from_secs(2);
        let mut captured = Vec::new();
        while captured.len() < clip.len() && Instant::now() < deadline {
            captured.extend_from_slice(&drain_all(&mut consumer));
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(
            captured.iter().any(|&s| (s - 0.5).abs() < 0.01),
            "clip audio never reached the ring"
        );

        // After the clip ends the feeder keeps pushing silence so subsequent
        // sessions still see a live stream.
        set_session(&transport, false);
        std::thread::sleep(Duration::from_millis(50));
        let tail = drain_all(&mut consumer);
        assert!(tail.iter().all(|&s| s == 0.0));

        drop(feeder);
    }

    #[test]
    fn feeder_rewinds_the_clip_for_each_new_session() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("clip.wav");
        let clip: Vec<i32> = vec![i16::MAX as i32 / 2; 1_600]; // 0.1 s
        write_wav(&path, spec(1, 16_000), &clip);

        let transport = Arc::new(CaptureTransportState::default());
        let (feeder, mut consumer, _rate) =
            FixtureFeeder::start_with_pace(&path, Arc::clone(&transport), false)
                .expect("start feeder");

        for _session in 0..2 {
            set_session(&transport, true);
            let deadline = Instant::now() + Duration::from_secs(2);
            let mut voiced = 0usize;
            let mut saw_voiced_after_start = false;
            while Instant::now() < deadline {
                for sample in drain_all(&mut consumer) {
                    if (sample - 0.5).abs() < 0.01 {
                        voiced += 1;
                        saw_voiced_after_start = true;
                    }
                }
                if voiced >= 1_600 {
                    break;
                }
                std::thread::sleep(Duration::from_millis(2));
            }
            assert!(saw_voiced_after_start, "clip never played");
            assert!(voiced >= 1_600, "clip truncated: {voiced} voiced samples");
            set_session(&transport, false);
            // Give the feeder a moment to notice the session ended so the next
            // session's rising edge is observed cleanly.
            std::thread::sleep(Duration::from_millis(20));
        }

        drop(feeder);
    }

    fn drain_all(consumer: &mut Consumer<f32>) -> Vec<f32> {
        let mut out = Vec::new();
        while consumer.slots() > 0 {
            let n = consumer.slots();
            let chunk = consumer.read_chunk(n).expect("readable");
            let (a, b) = chunk.as_slices();
            out.extend_from_slice(a);
            out.extend_from_slice(b);
            chunk.commit_all();
        }
        out
    }
}
