//! Which stretches of a sealed block go to the STT engine (FR-009-15).
//!
//! A block is transcribed in **one call** — all of its speech, internal
//! pauses included. The A/B harness (`ab_variants` + `scripts/meeting-ab.ps1`)
//! measured why on a real meeting: cutting a block into VAD utterances and
//! calling the engine per utterance lost 15% of them outright on Nemotron
//! (11 of 72 produced no text at all), made Whisper and Parakeet hallucinate
//! English filler at the cut edges ("Just legally.", "Yeah."), and split
//! sentences the engine could otherwise reconstruct — "Eu fico madrugada |
//! Maratona dando mão de série" against "Aí eu fico madruga / Fico
//! maratonando um monte de série". One call per block had zero edge
//! hallucinations and was three times faster, because 72 engine calls cost
//! more than one.
//!
//! So the VAD is no longer a knife; it answers two narrower questions:
//!
//! 1. **Is there any speech at all?** No voiced frame → no rows, the
//!    "nada ouvido" floor of FR-002-14 applied per block.
//! 2. **Where does the speech start and end?** Only the *outer* silence is
//!    trimmed, padded by [`PAD_MS`] on each side so the engine still hears
//!    the onset and the decay — dictation's `SmoothedVad` has always given
//!    it that pre-roll and hangover.
//!
//! Internal pauses stay inside the span: they are the context that lets the
//! engine finish a sentence.
//!
//! The one thing that still splits a block is a dictation interval
//! (AC-009-03): a `mic` block overlapping one is cut *at the interval
//! boundaries*, and the dictated stretch is persisted `excluded` as before.
//! Splitting there is the point — excluding a whole block because 3 s of it
//! were dictated would drop the meeting speech around it.

use crate::audio_toolkit::constants::WHISPER_SAMPLE_RATE;
use crate::audio_toolkit::vad::{MIN_SPEECH_DURATION_MS, VAD_PREFILL_MS};

/// Silence kept on each side of a block's speech. Dictation's pre-roll
/// (`VAD_PREFILL_MS`) and its offline hangover are the same 450 ms, so one
/// constant covers both edges.
pub(crate) const PAD_MS: u64 = VAD_PREFILL_MS;

/// One engine call's worth of a block, in block-local milliseconds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SpeechSpan {
    pub start_ms: u64,
    pub end_ms: u64,
    /// The span is inside a dictation interval: still transcribed, but
    /// persisted with `excluded = 1` (AC-009-03).
    pub excluded: bool,
}

impl SpeechSpan {
    /// Sample range into the block's buffer, clamped to `total_samples`.
    pub fn samples(&self, total_samples: usize) -> (usize, usize) {
        let rate = WHISPER_SAMPLE_RATE as usize;
        let start = (self.start_ms as usize * rate / 1_000).min(total_samples);
        let end = (self.end_ms as usize * rate / 1_000).min(total_samples);
        (start, end)
    }
}

/// First and last voiced instant in a block, in block-local ms — `None` when
/// the block holds no speech. `frame_ms` is the detector's frame length.
pub(crate) fn speech_bounds(frame_ms: u64, voiced: &[bool]) -> Option<(u64, u64)> {
    let first = voiced.iter().position(|v| *v)?;
    // `rposition` cannot fail once `position` found one.
    let last = voiced.iter().rposition(|v| *v)?;
    Some((first as u64 * frame_ms, (last as u64 + 1) * frame_ms))
}

/// The spans of one block that go to the engine.
///
/// `voiced` are the detector's per-frame verdicts, `frame_ms` its frame
/// length, `total_ms` the block's real duration (the last frame can be
/// partial, so the padded window clamps to it rather than to a frame edge),
/// and `dictation` the dictation intervals already clipped to this block and
/// expressed in block-local ms (empty for the `system` track, which
/// AC-009-03 never touches).
///
/// Returns at most one span when no dictation overlaps — the common case.
pub(crate) fn block_spans(
    total_ms: u64,
    frame_ms: u64,
    voiced: &[bool],
    dictation: &[(u64, u64)],
) -> Vec<SpeechSpan> {
    let Some((speech_start, speech_end)) = speech_bounds(frame_ms, voiced) else {
        return Vec::new();
    };
    let window_start = speech_start.saturating_sub(PAD_MS);
    let window_end = (speech_end + PAD_MS).min(total_ms);
    if window_end <= window_start {
        return Vec::new();
    }
    split_at_dictation(window_start, window_end, dictation)
}

/// The spans of a block whose speech bounds are unknown — the VAD failed to
/// initialise. The whole block goes to the engine, still split at dictation
/// boundaries: transcribing 60 s that may be mostly silence is better than
/// dropping real speech, and AC-009-03 holds either way.
pub(crate) fn split_whole_block(total_ms: u64, dictation: &[(u64, u64)]) -> Vec<SpeechSpan> {
    if total_ms == 0 {
        return Vec::new();
    }
    split_at_dictation(0, total_ms, dictation)
}

/// Cut `[start, end)` at the dictation boundaries inside it, tagging each
/// piece. Pieces shorter than [`MIN_SPEECH_DURATION_MS`] are dropped — the
/// same "nada ouvido" floor that keeps a noise blip out of the engine, here
/// keeping a sliver left between two dictations out of it.
fn split_at_dictation(start: u64, end: u64, dictation: &[(u64, u64)]) -> Vec<SpeechSpan> {
    let mut out = Vec::new();
    let mut cursor = start;
    let push = |start_ms: u64, end_ms: u64, excluded: bool, out: &mut Vec<SpeechSpan>| {
        if end_ms > start_ms && end_ms - start_ms >= MIN_SPEECH_DURATION_MS {
            out.push(SpeechSpan {
                start_ms,
                end_ms,
                excluded,
            });
        }
    };

    for &(dict_start, dict_end) in dictation {
        // Clip to the window; intervals outside it contribute nothing.
        let dict_start = dict_start.max(start);
        let dict_end = dict_end.min(end);
        if dict_end <= dict_start || dict_end <= cursor {
            continue;
        }
        push(cursor, dict_start, false, &mut out);
        push(dict_start.max(cursor), dict_end, true, &mut out);
        cursor = dict_end;
    }
    push(cursor, end, false, &mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 30 ms frames, like Silero.
    const FRAME_MS: u64 = 30;

    fn voiced_at(len: usize, range: std::ops::Range<usize>) -> Vec<bool> {
        (0..len).map(|i| range.contains(&i)).collect()
    }

    #[test]
    fn a_silent_block_produces_nothing() {
        assert!(block_spans(60_000, FRAME_MS, &[false; 100], &[]).is_empty());
        assert!(block_spans(60_000, FRAME_MS, &[], &[]).is_empty());
        assert_eq!(speech_bounds(FRAME_MS, &[false; 10]), None);
    }

    #[test]
    fn speech_bounds_cover_the_first_and_last_voiced_frame() {
        // Voiced frames 10..20 → 300 ms to 600 ms.
        let voiced = voiced_at(100, 10..20);
        assert_eq!(speech_bounds(FRAME_MS, &voiced), Some((300, 600)));
    }

    #[test]
    fn one_call_per_block_keeps_internal_pauses() {
        // Two speech islands with a long pause between them must come back as
        // ONE span: the pause is the context the engine uses to finish the
        // sentence, and cutting there is what the A/B run measured as worse.
        let mut voiced = vec![false; 2_000];
        voiced[100..150].fill(true);
        voiced[800..900].fill(true);
        let spans = block_spans(60_000, FRAME_MS, &voiced, &[]);
        assert_eq!(spans.len(), 1, "internal silence must not split a block");
        // 100*30 = 3000 ms speech start, minus 450 ms of pre-roll.
        assert_eq!(spans[0].start_ms, 3_000 - PAD_MS);
        // 900*30 = 27000 ms speech end, plus 450 ms of tail.
        assert_eq!(spans[0].end_ms, 27_000 + PAD_MS);
    }

    #[test]
    fn outer_silence_is_trimmed_with_padding_and_clamped() {
        // Speech from the very first frame: the pre-roll cannot go negative.
        let voiced = voiced_at(100, 0..10);
        let spans = block_spans(3_000, FRAME_MS, &voiced, &[]);
        assert_eq!(spans[0].start_ms, 0);

        // Speech running to the end: the tail clamps to the real duration,
        // never past the buffer.
        let voiced = voiced_at(100, 90..100);
        let spans = block_spans(3_000, FRAME_MS, &voiced, &[]);
        assert_eq!(spans[0].end_ms, 3_000);
    }

    #[test]
    fn a_dictation_interval_splits_the_block_instead_of_excluding_it() {
        // AC-009-03: the dictated stretch is excluded, the meeting speech
        // around it is not.
        let voiced = voiced_at(2_000, 0..2_000);
        let spans = block_spans(60_000, FRAME_MS, &voiced, &[(20_000, 30_000)]);
        assert_eq!(
            spans,
            vec![
                SpeechSpan {
                    start_ms: 0,
                    end_ms: 20_000,
                    excluded: false
                },
                SpeechSpan {
                    start_ms: 20_000,
                    end_ms: 30_000,
                    excluded: true
                },
                SpeechSpan {
                    start_ms: 30_000,
                    end_ms: 60_000,
                    excluded: false
                },
            ]
        );
    }

    #[test]
    fn dictation_covering_the_whole_block_yields_one_excluded_span() {
        let voiced = voiced_at(2_000, 0..2_000);
        let spans = block_spans(60_000, FRAME_MS, &voiced, &[(0, 60_000)]);
        assert_eq!(spans.len(), 1);
        assert!(spans[0].excluded);
        assert_eq!((spans[0].start_ms, spans[0].end_ms), (0, 60_000));
    }

    #[test]
    fn two_dictations_split_into_five_spans() {
        let voiced = voiced_at(2_000, 0..2_000);
        let spans = block_spans(
            60_000,
            FRAME_MS,
            &voiced,
            &[(10_000, 15_000), (40_000, 45_000)],
        );
        let shape: Vec<(u64, u64, bool)> = spans
            .iter()
            .map(|s| (s.start_ms, s.end_ms, s.excluded))
            .collect();
        assert_eq!(
            shape,
            vec![
                (0, 10_000, false),
                (10_000, 15_000, true),
                (15_000, 40_000, false),
                (40_000, 45_000, true),
                (45_000, 60_000, false),
            ]
        );
    }

    #[test]
    fn slivers_between_dictations_are_dropped() {
        // A 100 ms gap between two dictations is below the MIN_SPEECH floor —
        // it must not become its own engine call.
        let voiced = voiced_at(2_000, 0..2_000);
        let spans = block_spans(
            60_000,
            FRAME_MS,
            &voiced,
            &[(10_000, 20_000), (20_100, 30_000)],
        );
        let shape: Vec<(u64, u64, bool)> = spans
            .iter()
            .map(|s| (s.start_ms, s.end_ms, s.excluded))
            .collect();
        assert_eq!(
            shape,
            vec![
                (0, 10_000, false),
                (10_000, 20_000, true),
                // the 20_000..20_100 sliver is gone
                (20_100, 30_000, true),
                (30_000, 60_000, false),
            ]
        );
    }

    #[test]
    fn dictation_outside_the_speech_window_changes_nothing() {
        // Speech only in the first 3 s; a dictation at 40 s is outside the
        // trimmed window and must not produce an empty excluded span.
        let voiced = voiced_at(2_000, 0..100);
        let spans = block_spans(60_000, FRAME_MS, &voiced, &[(40_000, 50_000)]);
        assert_eq!(spans.len(), 1);
        assert!(!spans[0].excluded);
        assert_eq!(spans[0].end_ms, 3_000 + PAD_MS);
    }

    #[test]
    fn without_a_vad_the_whole_block_is_one_span() {
        let spans = split_whole_block(60_000, &[]);
        assert_eq!(
            spans,
            vec![SpeechSpan {
                start_ms: 0,
                end_ms: 60_000,
                excluded: false
            }]
        );
        // Dictation still splits it — AC-009-03 does not depend on the VAD.
        let spans = split_whole_block(60_000, &[(0, 10_000)]);
        assert_eq!(spans.len(), 2);
        assert!(spans[0].excluded);
        assert!(!spans[1].excluded);
        // An empty block yields nothing rather than a zero-length call.
        assert!(split_whole_block(0, &[]).is_empty());
    }

    #[test]
    fn sample_range_clamps_to_the_buffer() {
        let span = SpeechSpan {
            start_ms: 1_000,
            end_ms: 5_000,
            excluded: false,
        };
        // 16 kHz: 1 s = 16 000 samples.
        assert_eq!(span.samples(80_000), (16_000, 80_000));
        // A buffer shorter than the span clamps both ends.
        assert_eq!(span.samples(8_000), (8_000, 8_000));
    }
}
