use anyhow::Result;
use std::path::Path;

use vad_rs::Vad;

use super::{VadFrame, VoiceActivityDetector};
use crate::audio_toolkit::constants;

const SILERO_FRAME_MS: u32 = 30;
const SILERO_FRAME_SAMPLES: usize =
    (constants::WHISPER_SAMPLE_RATE * SILERO_FRAME_MS / 1000) as usize;

pub struct SileroVad {
    engine: Vad,
    threshold: f32,
    /// Voiced/fail-open frame count since `reset()` — see the trait docs.
    voiced_frames: usize,
}

impl SileroVad {
    pub fn new<P: AsRef<Path>>(model_path: P, threshold: f32) -> Result<Self> {
        if !(0.0..=1.0).contains(&threshold) {
            anyhow::bail!("threshold must be between 0.0 and 1.0");
        }

        Ok(Self {
            engine: Vad::new(&model_path, constants::WHISPER_SAMPLE_RATE as usize)
                .map_err(|e| anyhow::anyhow!("Failed to create VAD: {e}"))?,
            threshold,
            voiced_frames: 0,
        })
    }
}

impl VoiceActivityDetector for SileroVad {
    fn push_frame<'a>(&'a mut self, frame: &'a [f32]) -> Result<VadFrame<'a>> {
        if frame.len() != SILERO_FRAME_SAMPLES {
            // Fail-open: the recorder keeps errored frames as speech.
            self.voiced_frames += 1;
            anyhow::bail!(
                "expected {SILERO_FRAME_SAMPLES} samples, got {}",
                frame.len()
            );
        }

        let result = match self.engine.compute(frame) {
            Ok(result) => result,
            // Fail-open: the recorder keeps errored frames as speech, so they
            // count as voiced evidence for the "nada ouvido" floor.
            Err(e) => {
                self.voiced_frames += 1;
                anyhow::bail!("Silero VAD error: {e}");
            }
        };

        if result.prob > self.threshold {
            self.voiced_frames += 1;
            Ok(VadFrame::Speech(frame))
        } else {
            Ok(VadFrame::Noise)
        }
    }

    fn frame_samples(&self) -> usize {
        SILERO_FRAME_SAMPLES
    }

    fn voiced_frames(&self) -> usize {
        self.voiced_frames
    }

    fn reset(&mut self) {
        // Clear the Silero LSTM hidden/cell state so a new session doesn't
        // inherit recurrent context from the previous recording.
        self.engine.reset();
        self.voiced_frames = 0;
    }
}
