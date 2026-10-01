//! Utterance segmentation for live meeting transcription (FR-009-15).
//!
//! Sealed 60 s blocks are chopped into speech utterances of at most
//! [`MAX_UTTERANCE_MS`] (30 s) before they reach the STT engine. This module
//! holds the pure part — `voiced` frame verdicts in, utterance ranges out —
//! so the policy is testable without audio devices; the detector call lives
//! in `live::Runner::segment`.

use crate::audio_toolkit::constants::WHISPER_SAMPLE_RATE;
use crate::audio_toolkit::vad::{MIN_SPEECH_DURATION_MS, VAD_OFFLINE_HANGOVER_MS};

/// FR-009-15: VAD utterances handed to the meeting provider are ≤ 30 s.
pub const MAX_UTTERANCE_MS: u64 = 30_000;

/// Tuning for [`segment_utterances`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SegmenterConfig {
    /// Hard cap per utterance (FR-009-15 "blocos ≤ 30 s").
    pub max_ms: u64,
    /// Pause length that closes an utterance (the dictation pipeline's
    /// offline hangover — `VAD_OFFLINE_HANGOVER_MS`).
    pub end_silence_ms: u64,
    /// Voiced spans shorter than this are noise blips and never reach STT —
    /// the same floor as the "nada ouvido" check (FR-002-14).
    pub min_ms: u64,
}

impl Default for SegmenterConfig {
    fn default() -> Self {
        Self {
            max_ms: MAX_UTTERANCE_MS,
            end_silence_ms: VAD_OFFLINE_HANGOVER_MS,
            min_ms: MIN_SPEECH_DURATION_MS,
        }
    }
}

/// A detected speech span inside one block.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Utterance {
    /// Range into the block's sample array.
    pub start_sample: usize,
    pub end_sample: usize,
    /// Offsets from the block start, in milliseconds.
    pub start_ms: u64,
    pub end_ms: u64,
}

/// Turn per-frame voice decisions into utterance ranges — pure: the detector
/// call happens in the runner, so tests feed verdict vectors directly.
///
/// Rules: a pause of `cfg.end_silence_ms` closes an utterance; utterances
/// are capped at `cfg.max_ms`, splitting at the most recent internal pause
/// when one exists (never mid-word) and hard-cutting otherwise; spans shorter
/// than `cfg.min_ms` are dropped.
pub(crate) fn segment_utterances(
    total_samples: usize,
    frame_samples: usize,
    voiced: &[bool],
    cfg: &SegmenterConfig,
) -> Vec<Utterance> {
    if frame_samples == 0 || total_samples == 0 {
        return Vec::new();
    }
    let frame_ms = (frame_samples as u64 * 1_000 / WHISPER_SAMPLE_RATE as u64).max(1);
    let end_silence_frames = cfg.end_silence_ms.div_ceil(frame_ms).max(1) as usize;
    let max_frames = cfg.max_ms.div_ceil(frame_ms).max(2) as usize;
    let min_frames = cfg.min_ms.div_ceil(frame_ms).max(1) as usize;
    let n_frames = total_samples.div_ceil(frame_samples);
    let total_ms = total_samples as u64 * 1_000 / WHISPER_SAMPLE_RATE as u64;

    let make = |start: usize, end: usize| -> Option<Utterance> {
        (end - start >= min_frames).then(|| Utterance {
            start_sample: start * frame_samples,
            end_sample: end * frame_samples,
            start_ms: start as u64 * frame_ms,
            end_ms: (end as u64 * frame_ms).min(total_ms),
        })
    };

    let mut out = Vec::new();
    let mut utter_start: Option<usize> = None;
    // Frame index of the last voiced frame.
    let mut last_voiced = 0usize;
    // Consecutive non-voiced frames since `last_voiced`.
    let mut silence_run = 0usize;
    // Where the latest *internal* pause ended — the preferred split point
    // when the cap forces a cut.
    let mut last_gap_resume: Option<usize> = None;

    for i in 0..n_frames {
        if voiced.get(i).copied().unwrap_or(false) {
            match utter_start {
                None => {
                    utter_start = Some(i);
                    last_gap_resume = None;
                }
                Some(_) if silence_run > 0 => last_gap_resume = Some(i),
                _ => {}
            }
            silence_run = 0;
            last_voiced = i;
            if let Some(start) = utter_start {
                if i + 1 - start >= max_frames {
                    // Cap reached: prefer the last internal pause so a long
                    // take splits on silence rather than mid-word.
                    let cut = last_gap_resume.filter(|g| *g > start).unwrap_or(i + 1);
                    if let Some(u) = make(start, cut) {
                        out.push(u);
                    }
                    last_gap_resume = None;
                    // The voiced tail past the cut opens the next utterance.
                    utter_start = (cut <= i).then_some(cut);
                }
            }
        } else if utter_start.is_some() {
            silence_run += 1;
            if silence_run >= end_silence_frames {
                if let Some(u) = make(utter_start.unwrap_or(i), last_voiced + 1) {
                    out.push(u);
                }
                utter_start = None;
                last_gap_resume = None;
            }
        }
    }
    if let Some(start) = utter_start {
        if let Some(u) = make(start, last_voiced + 1) {
            out.push(u);
        }
    }
    out
}

/// Fallback when no VAD is available: chop the block into `max_ms` spans.
/// The engine-side hallucination filter still drops pure-silence output.
pub(crate) fn fixed_chunks(total_samples: usize, cfg: &SegmenterConfig) -> Vec<Utterance> {
    let chunk = (cfg.max_ms as usize * WHISPER_SAMPLE_RATE as usize / 1_000).max(1);
    (0..total_samples.div_ceil(chunk))
        .map(|i| Utterance {
            start_sample: i * chunk,
            end_sample: ((i + 1) * chunk).min(total_samples),
            start_ms: (i * chunk) as u64 * 1_000 / WHISPER_SAMPLE_RATE as u64,
            end_ms: (((i + 1) * chunk).min(total_samples)) as u64 * 1_000
                / WHISPER_SAMPLE_RATE as u64,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 30 ms frames (Silero) with ms-level config: silence ≥ 90 ms closes an
    /// utterance, cap at 300 ms, drop spans < 60 ms.
    fn test_cfg() -> SegmenterConfig {
        SegmenterConfig {
            max_ms: 300,
            end_silence_ms: 90,
            min_ms: 60,
        }
    }

    const FRAME: usize = 480; // 30 ms at 16 kHz

    fn seg(samples: usize, voiced: &[bool]) -> Vec<Utterance> {
        segment_utterances(samples, FRAME, voiced, &test_cfg())
    }

    #[test]
    fn silence_produces_no_utterances() {
        assert!(seg(10 * FRAME, &[false; 10]).is_empty());
        assert!(seg(0, &[true; 4]).is_empty(), "no samples → nothing");
    }

    #[test]
    fn voiced_span_shorter_than_min_is_dropped() {
        // 1 voiced frame (30 ms) < 60 ms floor.
        let mut voiced = vec![false; 10];
        voiced[4] = true;
        assert!(seg(10 * FRAME, &voiced).is_empty());
    }

    #[test]
    fn pause_longer_than_end_silence_splits_utterances() {
        // 3 voiced (90 ms) | 4 silent (120 ms ≥ 90 ms) | 3 voiced.
        let voiced = [
            true, true, true, false, false, false, false, true, true, true,
        ];
        let utterances = seg(10 * FRAME, &voiced);
        assert_eq!(utterances.len(), 2);
        assert_eq!((utterances[0].start_ms, utterances[0].end_ms), (0, 90));
        assert_eq!((utterances[1].start_ms, utterances[1].end_ms), (210, 300));
        assert_eq!(utterances[0].start_sample, 0);
        assert_eq!(utterances[0].end_sample, 3 * FRAME);
    }

    #[test]
    fn short_pause_does_not_split() {
        // 2 silent frames (60 ms < 90 ms) inside a voiced span.
        let voiced = [true, true, true, false, false, true, true, true];
        let utterances = seg(8 * FRAME, &voiced);
        assert_eq!(utterances.len(), 1);
        assert_eq!((utterances[0].start_ms, utterances[0].end_ms), (0, 240));
    }

    #[test]
    fn continuous_speech_hard_splits_at_the_cap() {
        // 25 voiced frames (750 ms) with a 300 ms cap → 10/10/5.
        let voiced = vec![true; 25];
        let utterances = seg(25 * FRAME, &voiced);
        assert_eq!(utterances.len(), 3);
        assert_eq!(utterances[0].end_ms - utterances[0].start_ms, 300);
        assert_eq!(utterances[1].start_ms, 300);
        assert_eq!(utterances[1].end_ms, 600);
        assert_eq!((utterances[2].start_ms, utterances[2].end_ms), (600, 750));
    }

    #[test]
    fn cap_prefers_splitting_at_an_internal_pause() {
        // Voiced 0..6, pause 6..8 (60 ms — too short to close), voiced 8..12.
        // Cap = 10 frames fires at i=9; the internal pause ended at 8.
        let voiced = [
            true, true, true, true, true, true, false, false, true, true, true, true,
        ];
        let utterances = seg(12 * FRAME, &voiced);
        assert_eq!(utterances.len(), 2);
        assert_eq!((utterances[0].start_ms, utterances[0].end_ms), (0, 240));
        assert_eq!((utterances[1].start_ms, utterances[1].end_ms), (240, 360));
    }

    #[test]
    fn trailing_utterance_is_flushed_at_block_end() {
        let mut voiced = vec![false; 8];
        voiced[6] = true;
        voiced[7] = true; // 60 ms tail — no closing silence follows.
        let utterances = seg(8 * FRAME, &voiced);
        assert_eq!(utterances.len(), 1);
        assert_eq!((utterances[0].start_ms, utterances[0].end_ms), (180, 240));
    }

    #[test]
    fn partial_last_frame_counts_and_end_ms_clamps() {
        // 10.5 frames of audio, all voiced, cap 300 ms → one 10-frame
        // utterance plus a half-frame tail < min → dropped.
        let voiced = vec![true; 11];
        let utterances = seg(10 * FRAME + FRAME / 2, &voiced);
        assert_eq!(utterances.len(), 1);
        assert_eq!(utterances[0].end_ms, 300);
    }

    #[test]
    fn fixed_chunks_fallback_respects_the_cap() {
        let cfg = SegmenterConfig {
            max_ms: 1_000,
            ..test_cfg()
        };
        let chunks = fixed_chunks(80_000, &cfg); // 5 s of 16 kHz audio
        assert_eq!(chunks.len(), 5);
        assert_eq!(chunks[0].end_sample - chunks[0].start_sample, 16_000);
        assert_eq!(chunks[4].start_ms, 4_000);
    }
}
