use anyhow::Result;

use super::{VadFrame, VoiceActivityDetector};

pub const EARSHOT_FRAME_SAMPLES: usize = 256;

/// Pure-Rust Earshot VAD adapter.
///
/// Earshot expects exactly 16 ms of mono 16 kHz audio per prediction. The
/// recorder uses `frame_samples()` to configure its resampler accordingly.
pub struct EarshotVad {
    engine: Box<earshot::Detector>,
    threshold: f32,
    clamped_frame: [f32; EARSHOT_FRAME_SAMPLES],
    /// Voiced/fail-open frame count since `reset()` — see the trait docs.
    voiced_frames: usize,
}

impl EarshotVad {
    pub fn new(threshold: f32) -> Result<Self> {
        if !(0.0..=1.0).contains(&threshold) {
            anyhow::bail!("threshold must be between 0.0 and 1.0");
        }

        Ok(Self {
            // Construct directly on the heap: Detector keeps roughly 8 KiB of
            // model state and scratch buffers.
            engine: earshot::Detector::default_boxed(),
            threshold,
            clamped_frame: [0.0; EARSHOT_FRAME_SAMPLES],
            voiced_frames: 0,
        })
    }
}

impl VoiceActivityDetector for EarshotVad {
    fn push_frame<'a>(&'a mut self, frame: &'a [f32]) -> Result<VadFrame<'a>> {
        if frame.len() != EARSHOT_FRAME_SAMPLES {
            // Fail-open: the recorder keeps errored frames as speech.
            self.voiced_frames += 1;
            anyhow::bail!(
                "expected {EARSHOT_FRAME_SAMPLES} samples, got {}",
                frame.len()
            );
        }
        if frame.iter().any(|sample| !sample.is_finite()) {
            // Fail-open: the recorder keeps errored frames as speech, so they
            // count as voiced evidence for the "nada ouvido" floor.
            self.voiced_frames += 1;
            anyhow::bail!("Earshot VAD input contained a non-finite sample");
        }

        // cpal produces normalized f32 samples, but resampling a full-scale
        // signal can ring slightly outside [-1, 1]. Earshot documents that
        // range as a precondition and asserts it in debug builds, so clamp only
        // the prediction input while preserving the original audio on output.
        let score = if frame.iter().all(|sample| (-1.0..=1.0).contains(sample)) {
            self.engine.predict_f32(frame)
        } else {
            for (clamped, sample) in self.clamped_frame.iter_mut().zip(frame) {
                *clamped = sample.clamp(-1.0, 1.0);
            }
            self.engine.predict_f32(&self.clamped_frame)
        };

        if score >= self.threshold {
            self.voiced_frames += 1;
            Ok(VadFrame::Speech(frame))
        } else {
            Ok(VadFrame::Noise)
        }
    }

    fn frame_samples(&self) -> usize {
        EARSHOT_FRAME_SAMPLES
    }

    fn voiced_frames(&self) -> usize {
        self.voiced_frames
    }

    fn reset(&mut self) {
        self.engine.reset();
        self.voiced_frames = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_wrong_frame_size() {
        let mut vad = EarshotVad::new(0.5).unwrap();
        assert!(vad.push_frame(&[0.0; EARSHOT_FRAME_SAMPLES - 1]).is_err());
    }

    #[test]
    fn silence_is_not_voice() {
        let mut vad = EarshotVad::new(0.5).unwrap();
        assert!(!vad
            .push_frame(&[0.0; EARSHOT_FRAME_SAMPLES])
            .unwrap()
            .is_speech());
    }

    #[test]
    fn validates_threshold() {
        assert!(EarshotVad::new(-0.1).is_err());
        assert!(EarshotVad::new(1.1).is_err());
    }

    #[test]
    fn clamps_resampler_overshoot_without_changing_output() {
        let mut vad = EarshotVad::new(0.0).unwrap();
        let frame = [1.01; EARSHOT_FRAME_SAMPLES];
        match vad.push_frame(&frame).unwrap() {
            VadFrame::Speech(output) => assert_eq!(output, frame),
            VadFrame::Noise => panic!("zero threshold should retain the frame"),
        }
    }

    #[test]
    fn rejects_non_finite_audio() {
        let mut vad = EarshotVad::new(0.5).unwrap();
        let mut frame = [0.0; EARSHOT_FRAME_SAMPLES];
        frame[0] = f32::NAN;
        assert!(vad.push_frame(&frame).is_err());
    }

    #[test]
    fn voiced_frames_counts_speech_and_fail_open_errors_only() {
        let mut vad = EarshotVad::new(0.5).unwrap();
        vad.push_frame(&[0.0; EARSHOT_FRAME_SAMPLES]).unwrap();
        assert_eq!(vad.voiced_frames(), 0, "silence is not voiced");

        // A rejected frame is emitted downstream as speech (fail-open), so it
        // counts as voiced evidence.
        let mut bad = [0.0; EARSHOT_FRAME_SAMPLES];
        bad[0] = f32::INFINITY;
        assert!(vad.push_frame(&bad).is_err());
        assert_eq!(vad.voiced_frames(), 1);

        vad.reset();
        assert_eq!(vad.voiced_frames(), 0);
    }
}
