use rustfft::{num_complex::Complex32, Fft, FftPlanner};
use std::sync::Arc;

// `db` below is not true dBFS: it's a per-bin average divided by the FFT
// window size, which lands well below absolute input level. The adaptive floor
// tracks persistent room tone and adds headroom, so only a signal clearly above
// the measured noise floor can lift the Flow Bar. The old -55/-8 range left
// speech ~1 px above the overlay floor, which reads as a frozen waveform
// (#1694).
const DB_MIN: f32 = -68.0;
const DB_MAX: f32 = -30.0;
const NOISE_TRACK_MAX_DB: f32 = -56.0;
const NOISE_HEADROOM_DB: f32 = 8.0;
const GAIN: f32 = 1.3;
const CURVE_POWER: f32 = 0.7;

pub struct AudioVisualiser {
    fft: Arc<dyn Fft<f32>>,
    window: Vec<f32>,
    bucket_ranges: Vec<(usize, usize)>,
    fft_input: Vec<Complex32>,
    noise_floor: Vec<f32>,
    buffer: Vec<f32>,
    window_size: usize,
    buckets: usize,
}

impl AudioVisualiser {
    pub fn new(
        sample_rate: u32,
        window_size: usize,
        buckets: usize,
        freq_min: f32,
        freq_max: f32,
    ) -> Self {
        let mut planner = FftPlanner::<f32>::new();
        let fft = planner.plan_fft_forward(window_size);

        // Pre-compute Hann window
        let window: Vec<f32> = (0..window_size)
            .map(|i| {
                0.5 * (1.0 - (2.0 * std::f32::consts::PI * i as f32 / window_size as f32).cos())
            })
            .collect();

        // Pre-compute bucket frequency ranges
        let nyquist = sample_rate as f32 / 2.0;
        let freq_min = freq_min.min(nyquist);
        let freq_max = freq_max.min(nyquist);

        let mut bucket_ranges = Vec::with_capacity(buckets);

        for b in 0..buckets {
            // Use logarithmic spacing for better perceptual representation
            let log_start = (b as f32 / buckets as f32).powi(2);
            let log_end = ((b + 1) as f32 / buckets as f32).powi(2);

            let start_hz = freq_min + (freq_max - freq_min) * log_start;
            let end_hz = freq_min + (freq_max - freq_min) * log_end;

            let start_bin = ((start_hz * window_size as f32) / sample_rate as f32) as usize;
            let mut end_bin = ((end_hz * window_size as f32) / sample_rate as f32) as usize;

            // Ensure each bucket has at least one bin
            if end_bin <= start_bin {
                end_bin = start_bin + 1;
            }

            // Clamp to valid range
            let start_bin = start_bin.min(window_size / 2);
            let end_bin = end_bin.min(window_size / 2);

            bucket_ranges.push((start_bin, end_bin));
        }

        Self {
            fft,
            window,
            bucket_ranges,
            fft_input: vec![Complex32::new(0.0, 0.0); window_size],
            // Start at the absolute floor. Persistent quiet input then adapts
            // the per-band floor upward instead of leaving a fixed threshold
            // that every headset has to fit.
            noise_floor: vec![DB_MIN; buckets],
            buffer: Vec::with_capacity(window_size * 2),
            window_size,
            buckets,
        }
    }

    pub fn feed(&mut self, samples: &[f32]) -> Option<Vec<f32>> {
        // Add new samples to buffer
        self.buffer.extend_from_slice(samples);

        // Only process if we have enough samples
        if self.buffer.len() < self.window_size {
            return None;
        }

        // Take the required window of samples
        let window_samples = &self.buffer[..self.window_size];

        // Remove DC component
        let mean = window_samples.iter().sum::<f32>() / self.window_size as f32;

        // Apply window function and prepare FFT input
        for (i, &sample) in window_samples.iter().enumerate() {
            let windowed_sample = (sample - mean) * self.window[i];
            self.fft_input[i] = Complex32::new(windowed_sample, 0.0);
        }

        // Perform FFT
        self.fft.process(&mut self.fft_input);

        // Compute power spectrum and bucket levels
        let mut buckets = vec![0.0; self.buckets];

        for (bucket_idx, &(start_bin, end_bin)) in self.bucket_ranges.iter().enumerate() {
            if start_bin >= end_bin || end_bin > self.fft_input.len() / 2 {
                continue;
            }

            // Calculate average power in this frequency range
            let mut power_sum = 0.0;
            for bin_idx in start_bin..end_bin {
                let magnitude = self.fft_input[bin_idx].norm();
                power_sum += magnitude * magnitude;
            }

            let avg_power = power_sum / (end_bin - start_bin) as f32;

            // Convert to dB with proper scaling
            let db = if avg_power > 1e-12 {
                20.0 * (avg_power.sqrt() / self.window_size as f32).log10()
            } else {
                -80.0 // Very low floor for zero power
            };

            // Track persistent quiet input as room tone, but never let a
            // speech-band transient raise the floor. A signal must clear the
            // floor plus headroom before it can animate a bar.
            if db < NOISE_TRACK_MAX_DB {
                const NOISE_ALPHA: f32 = 0.01; // ~100 windows to adapt
                self.noise_floor[bucket_idx] =
                    NOISE_ALPHA * db + (1.0 - NOISE_ALPHA) * self.noise_floor[bucket_idx];
            }
            let floor_gate =
                (self.noise_floor[bucket_idx] + NOISE_HEADROOM_DB).clamp(DB_MIN, DB_MAX - 1.0);

            // Map the adaptive dB range to 0-1 with gain and curve shaping.
            let normalized = ((db - floor_gate) / (DB_MAX - floor_gate)).clamp(0.0, 1.0);
            buckets[bucket_idx] = (normalized * GAIN).powf(CURVE_POWER).clamp(0.0, 1.0);
        }

        // Apply light smoothing to reduce jitter
        for i in 1..buckets.len() - 1 {
            buckets[i] = buckets[i] * 0.7 + buckets[i - 1] * 0.15 + buckets[i + 1] * 0.15;
        }

        // Clear processed samples from buffer
        self.buffer.clear();

        Some(buckets)
    }

    pub fn reset(&mut self) {
        self.buffer.clear();
        self.noise_floor.fill(DB_MIN);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 48_000;
    const BUCKETS: usize = 16;

    fn sine(amplitude: f32, freq: f32, len: usize) -> Vec<f32> {
        (0..len)
            .map(|i| amplitude * (2.0 * std::f32::consts::PI * freq * i as f32 / RATE as f32).sin())
            .collect()
    }

    /// Mirror of `CaptureProcessor::new`: window closest to sample_rate/30 so
    /// `feed` yields one emission per ~33 ms (FR-001-05's ~30 Hz level stream).
    fn visualizer_for(rate: u32) -> AudioVisualiser {
        let target_window = (rate as f64 / 30.0).round() as usize;
        let window_size = [256usize, 512, 1024, 2048]
            .into_iter()
            .min_by_key(|w| w.abs_diff(target_window))
            .expect("window list is non-empty");
        AudioVisualiser::new(rate, window_size, BUCKETS, 400.0, 4000.0)
    }

    #[test]
    fn feed_waits_for_a_full_window_before_emitting() {
        let mut vis = visualizer_for(RATE);
        assert!(vis.feed(&vec![0.0f32; 512]).is_none());
    }

    #[test]
    fn feed_emits_at_roughly_30_hz() {
        // 48 kHz -> 2048-sample window -> an emission every ~42.7 ms of audio
        // input; emit_levels then caps IPC at ~30 Hz. One second of audio must
        // produce a cadence in that ballpark, not one-per-callback bursts.
        let mut vis = visualizer_for(RATE);
        let chunk = vec![0.0f32; 480]; // 10 ms drains, as run_consumer produces
        let mut emissions = 0;
        for _ in 0..100 {
            if vis.feed(&chunk).is_some() {
                emissions += 1;
            }
        }
        assert!(
            (15..=50).contains(&emissions),
            "expected ~30 Hz metering over one second, got {emissions} emissions"
        );
    }

    #[test]
    fn silence_stays_at_the_bars_floor() {
        let mut vis = visualizer_for(RATE);
        let buckets = vis
            .feed(&vec![0.0f32; 2048])
            .expect("a full window emits buckets");
        assert_eq!(buckets.len(), BUCKETS);
        assert!(
            buckets.iter().all(|&b| b <= 0.05),
            "silence should pin every bar near zero: {buckets:?}"
        );
    }

    #[test]
    fn room_tone_below_the_speech_floor_stays_flat() {
        let mut vis = visualizer_for(RATE);
        // A ~-52 dBFS in-band tone is a generous stand-in for a headset's room
        // tone: audible to the meter, but not user speech. It must not animate
        // the Flow Bar by itself.
        let buckets = vis
            .feed(&sine(0.0025, 1000.0, 2048))
            .expect("a full window emits buckets");
        assert!(
            buckets.iter().all(|&b| b <= 0.05),
            "room tone should pin every bar near zero: {buckets:?}"
        );
    }

    #[test]
    fn a_voice_band_tone_lifts_some_bars() {
        let mut vis = visualizer_for(RATE);
        // 1 kHz sits inside the 400-4000 Hz metering band; a full-scale tone
        // must visibly move at least one bar (AC-001-07).
        let buckets = vis
            .feed(&sine(0.8, 1000.0, 2048))
            .expect("a full window emits buckets");
        assert!(
            buckets.iter().any(|&b| b > 0.2),
            "a strong in-band tone should lift some bars: {buckets:?}"
        );
    }

    #[test]
    fn reset_discards_partially_accumulated_audio() {
        let mut vis = visualizer_for(RATE);
        assert!(vis.feed(&sine(0.8, 1000.0, 1024)).is_none());
        vis.reset();
        // If the partial window had survived reset, this 1024-sample feed
        // would complete a window using stale audio.
        assert!(vis.feed(&vec![0.0f32; 1024]).is_none());
    }
}
