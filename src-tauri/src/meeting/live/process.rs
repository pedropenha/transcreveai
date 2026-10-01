//! Per-block utterance pass — the pure orchestration between VAD output and
//! segment persistence so the retry/skip/exclusion policy is testable
//! without a real STT engine.

use super::segment::Utterance;
use crate::meeting::blocks::SealedBlock;
use crate::stt::types::SttError;

/// How one pass over a block's utterances ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BlockPass {
    /// Every utterance was attempted — mark `transcribed = 1`.
    Done,
    /// An STT failure cut the pass short — the block stays pending for
    /// T-067 (already-written utterances of this block can duplicate; the
    /// post-pass owns dedup, see the `live` module docs).
    Failed,
    /// The engine was busy/not loaded — requeue and resume at this
    /// utterance index.
    Busy(usize),
}

/// Run a block's utterances through `transcribe`; surviving text goes to
/// `insert(start_ms, end_ms, text, excluded)`. Pure orchestration — the
/// production caller supplies the provider call and the row insert; tests
/// script both.
///
/// `is_excluded(start_ms, end_ms)` is the dictation-overlap test (the
/// production closure returns `false` for `system` blocks — AC-009-03's
/// "system track continues normally"). The orchestrator's own single retry
/// already ran inside `transcribe`, so an `Err` here is terminal for the
/// block — except `SttError::ModelNotReady`, which means "engine busy",
/// yielding [`BlockPass::Busy`].
pub(crate) fn process_utterances(
    block: &SealedBlock,
    utterances: &[Utterance],
    resume_from: usize,
    samples: &[f32],
    transcribe: &mut dyn FnMut(&[f32]) -> std::result::Result<String, SttError>,
    insert: &mut dyn FnMut(i64, i64, &str, bool),
    is_excluded: &dyn Fn(i64, i64) -> bool,
) -> BlockPass {
    for (i, u) in utterances.iter().enumerate().skip(resume_from) {
        let start_ms = block.start_offset_ms as i64 + u.start_ms as i64;
        let end_ms = block.start_offset_ms as i64 + u.end_ms as i64;
        let end_sample = u.end_sample.min(samples.len());
        if u.start_sample >= end_sample {
            continue;
        }
        match transcribe(&samples[u.start_sample..end_sample]) {
            Ok(text) => {
                let text = text.trim();
                if !text.is_empty() {
                    insert(start_ms, end_ms, text, is_excluded(start_ms, end_ms));
                }
            }
            Err(SttError::ModelNotReady) => return BlockPass::Busy(i),
            Err(_) => return BlockPass::Failed,
        }
    }
    BlockPass::Done
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::meeting::blocks::Track;
    use crate::meeting::live::dictation::DictationTracker;
    use std::path::PathBuf;

    fn block(track: Track) -> SealedBlock {
        SealedBlock {
            track,
            index: 1,
            path: PathBuf::new(),
            start_offset_ms: 10_000,
            duration_ms: 60_000,
            samples: 960_000,
        }
    }

    /// Two 1 s utterances: [0, 16 000) and [32 000, 48 000).
    fn two_utterances() -> Vec<Utterance> {
        vec![
            Utterance {
                start_sample: 0,
                end_sample: 16_000,
                start_ms: 0,
                end_ms: 1_000,
            },
            Utterance {
                start_sample: 32_000,
                end_sample: 48_000,
                start_ms: 2_000,
                end_ms: 3_000,
            },
        ]
    }

    #[test]
    fn pass_inserts_segments_with_meeting_offsets() {
        let samples = vec![0.0f32; 960_000];
        let mut inserted = Vec::new();
        let pass = process_utterances(
            &block(Track::Mic),
            &two_utterances(),
            0,
            &samples,
            &mut |_| Ok("  texto ".to_string()),
            &mut |s, e, t, x| inserted.push((s, e, t.to_string(), x)),
            &|_, _| false,
        );
        assert_eq!(pass, BlockPass::Done);
        assert_eq!(
            inserted,
            vec![
                (10_000, 11_000, "texto".to_string(), false),
                (12_000, 13_000, "texto".to_string(), false),
            ],
            "block offset is added; text is trimmed before insert"
        );
    }

    #[test]
    fn pass_marks_mic_utterances_overlapping_dictation() {
        let mut tracker = DictationTracker::default();
        tracker.begin(11_500);
        tracker.end(13_000);
        let samples = vec![0.0f32; 960_000];
        let mut inserted = Vec::new();
        process_utterances(
            &block(Track::Mic),
            &two_utterances(),
            0,
            &samples,
            &mut |_| Ok("t".to_string()),
            &mut |s, e, t, x| inserted.push((s, e, t.to_string(), x)),
            &|s, e| tracker.overlaps(s, e),
        );
        assert_eq!(
            inserted.iter().map(|i| i.3).collect::<Vec<_>>(),
            vec![false, true],
            "only the utterance inside the dictation interval is excluded"
        );
    }

    #[test]
    fn pass_never_excludes_system_track() {
        let mut tracker = DictationTracker::default();
        tracker.begin(0);
        tracker.end(120_000);
        let samples = vec![0.0f32; 960_000];
        let mut excluded = Vec::new();
        // Production wires `is_excluded` to return false for system blocks —
        // simulate the real closure's track gate here.
        let is_mic = false;
        process_utterances(
            &block(Track::System),
            &two_utterances(),
            0,
            &samples,
            &mut |_| Ok("t".to_string()),
            &mut |_, _, _, x| excluded.push(x),
            &move |s, e| is_mic && tracker.overlaps(s, e),
        );
        assert!(excluded.iter().all(|x| !*x));
    }

    #[test]
    fn pass_model_not_ready_reports_resume_index() {
        let samples = vec![0.0f32; 960_000];
        let mut calls = 0usize;
        let pass = process_utterances(
            &block(Track::Mic),
            &two_utterances(),
            0,
            &samples,
            &mut |_| {
                calls += 1;
                if calls == 2 {
                    Err(SttError::ModelNotReady)
                } else {
                    Ok("t".to_string())
                }
            },
            &mut |_, _, _, _| {},
            &|_, _| false,
        );
        assert_eq!(pass, BlockPass::Busy(1));
    }

    #[test]
    fn pass_failure_skips_the_block_and_resume_skips_done_utterances() {
        let samples = vec![0.0f32; 960_000];
        // First pass: first utterance ok, second fails → Failed (the single
        // retry already happened inside the scripted transcribe boundary).
        let mut inserted = Vec::new();
        let mut calls = 0usize;
        let pass = process_utterances(
            &block(Track::Mic),
            &two_utterances(),
            0,
            &samples,
            &mut |_| {
                calls += 1;
                if calls == 2 {
                    Err(SttError::Provider("engine".to_string()))
                } else {
                    Ok("t".to_string())
                }
            },
            &mut |s, _, _, _| inserted.push(s),
            &|_, _| false,
        );
        assert_eq!(pass, BlockPass::Failed);
        assert_eq!(inserted, vec![10_000]);

        // Resume at 1 → only the remaining utterance runs.
        let mut resumed = Vec::new();
        process_utterances(
            &block(Track::Mic),
            &two_utterances(),
            1,
            &samples,
            &mut |_| Ok("t2".to_string()),
            &mut |s, _, _, _| resumed.push(s),
            &|_, _| false,
        );
        assert_eq!(resumed, vec![12_000]);
    }

    #[test]
    fn pass_empty_text_inserts_nothing() {
        let samples = vec![0.0f32; 960_000];
        let mut inserted = 0usize;
        let pass = process_utterances(
            &block(Track::Mic),
            &two_utterances(),
            0,
            &samples,
            &mut |_| Ok("   ".to_string()),
            &mut |_, _, _, _| inserted += 1,
            &|_, _| false,
        );
        assert_eq!(pass, BlockPass::Done);
        assert_eq!(inserted, 0);
    }
}
