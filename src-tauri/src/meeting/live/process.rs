//! Per-block utterance pass — the pure orchestration between VAD output and
//! segment persistence so the retry/skip/exclusion policy is testable
//! without a real STT engine.

use super::segment::Utterance;
use crate::audio_toolkit::constants::WHISPER_SAMPLE_RATE;
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
///
/// Meeting-clock ms ranges a live pass already persisted — kept sorted,
/// merged and non-overlapping. Live chunks and sealing blocks both dedup
/// against this set: only the still-uncovered portions of each utterance
/// are transcribed, so a dropped chunk leaves a hole the block re-covers
/// instead of a watermark skipping every later utterance.
pub(crate) type Coverage = Vec<(i64, i64)>;

/// Fold `[start, end)` into `covered`, merging every interval it touches.
/// Recorded per inserted portion so a busy retry's recomputed
/// [`uncovered`] skips what already landed.
pub(crate) fn record_coverage(covered: &mut Coverage, start: i64, end: i64) {
    if end <= start {
        return;
    }
    // First interval whose end reaches `start`; everything overlapping
    // `[start, end)` from there merges into one.
    let first = covered.partition_point(|&(_, b)| b < start);
    let mut merged_start = start;
    let mut merged_end = end;
    let mut last = first;
    while let Some(&(a, b)) = covered.get(last) {
        if a > end {
            break;
        }
        merged_start = merged_start.min(a);
        merged_end = merged_end.max(b);
        last += 1;
    }
    covered.splice(first..last, [(merged_start, merged_end)]);
}

/// `[start, end)` minus `covered` — the still-untranscribed portions, in
/// order. An empty result means the range is already persisted.
pub(crate) fn uncovered(covered: &Coverage, start: i64, end: i64) -> Vec<(i64, i64)> {
    let mut out = Vec::new();
    let mut cursor = start;
    for &(a, b) in covered {
        if b <= cursor {
            continue;
        }
        if a >= end {
            break;
        }
        if a > cursor {
            out.push((cursor, a.min(end)));
        }
        cursor = cursor.max(b);
        if cursor >= end {
            break;
        }
    }
    if cursor < end {
        out.push((cursor, end));
    }
    out
}

/// A live chunk that ends mid-utterance must not persist the clipped
/// half: drop the open tail — the sealing block re-covers it — and
/// return the samples before it, which is all the chunk may mark
/// covered. `tail_voiced` guards against popping a properly closed final
/// utterance; the `>=` on `end_sample` tolerates a segmenter that emits
/// frame-aligned (unclamped) endpoints. Returns `total_samples` when
/// nothing is dropped.
pub(crate) fn drop_open_tail(
    utterances: &mut Vec<Utterance>,
    tail_voiced: bool,
    total_samples: usize,
) -> usize {
    if !tail_voiced {
        return total_samples;
    }
    match utterances.last() {
        Some(last) if last.end_sample >= total_samples => {
            let covered = last.start_sample;
            utterances.pop();
            covered
        }
        _ => total_samples,
    }
}

/// Where a pass (re)starts: `utterance` is the busy-retry cursor —
/// utterances before it completed every portion on the earlier pass.
/// Dedup itself lives in [`Coverage`], passed separately.
#[derive(Clone, Copy, Default)]
pub(crate) struct ResumePoint {
    pub utterance: usize,
}

/// The "what is already done" state a pass dedups against: the busy-retry
/// cursor plus the per-track interval set live chunks and sealing blocks
/// have already persisted.
pub(crate) struct Dedup<'a> {
    pub resume: ResumePoint,
    pub covered: &'a mut Coverage,
}

/// Uncovered portions narrower than this are boundary slivers: chunk and
/// block ranges floor-round to whole ms, so adjacent spans can leave 1–40
/// ms gaps. Transcribing them wastes a pass, can fail a whole block on a
/// provider hiccup over 16 samples of audio, or insert a hallucinated
/// word — coverage marks them settled instead.
const MIN_PORTION_MS: i64 = 250;

pub(crate) fn process_utterances(
    block: &SealedBlock,
    utterances: &[Utterance],
    dedup: Dedup<'_>,
    samples: &[f32],
    transcribe: &mut dyn FnMut(&[f32]) -> std::result::Result<String, SttError>,
    // `insert` persists one portion and returns `false` when the row did
    // not land — the portion stays uncovered and the pass fails so the
    // block keeps its pending row for post-processing.
    insert: &mut dyn FnMut(i64, i64, &str, bool) -> bool,
    is_excluded: &dyn Fn(i64, i64) -> bool,
) -> BlockPass {
    let covered = dedup.covered;
    for (i, u) in utterances.iter().enumerate().skip(dedup.resume.utterance) {
        let utterance_start_ms = block.start_offset_ms as i64 + u.start_ms as i64;
        let utterance_end_ms = block.start_offset_ms as i64 + u.end_ms as i64;
        // Live chunks and blocks split differently, and a dropped chunk
        // leaves a hole in the coverage set — each portion is the part of
        // this utterance that still needs transcription. The row and the
        // transcribed samples keep only that meeting-clock range.
        for (start_ms, end_ms) in uncovered(covered, utterance_start_ms, utterance_end_ms) {
            if end_ms - start_ms < MIN_PORTION_MS {
                record_coverage(covered, start_ms, end_ms);
                continue;
            }
            let start_sample = u.start_sample
                + (start_ms - utterance_start_ms) as usize * WHISPER_SAMPLE_RATE as usize / 1_000;
            let end_sample = (u.start_sample
                + (end_ms - utterance_start_ms) as usize * WHISPER_SAMPLE_RATE as usize / 1_000)
                .min(samples.len());
            if start_sample >= end_sample {
                continue;
            }
            match transcribe(&samples[start_sample..end_sample]) {
                Ok(text) => {
                    let text = text.trim();
                    // Coverage means "settled — never re-transcribe": an
                    // empty transcript counts, or a busy retry / sealing
                    // block would re-run STT on silence forever.
                    if text.is_empty() {
                        record_coverage(covered, start_ms, end_ms);
                        continue;
                    }
                    if insert(start_ms, end_ms, text, is_excluded(start_ms, end_ms)) {
                        record_coverage(covered, start_ms, end_ms);
                    } else {
                        return BlockPass::Failed;
                    }
                }
                Err(SttError::ModelNotReady) => return BlockPass::Busy(i),
                Err(_) => return BlockPass::Failed,
            }
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
            Dedup {
                resume: ResumePoint::default(),
                covered: &mut Vec::new(),
            },
            &samples,
            &mut |_| Ok("  texto ".to_string()),
            &mut |s, e, t, x| {
                inserted.push((s, e, t.to_string(), x));
                true
            },
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
            Dedup {
                resume: ResumePoint::default(),
                covered: &mut Vec::new(),
            },
            &samples,
            &mut |_| Ok("t".to_string()),
            &mut |s, e, t, x| {
                inserted.push((s, e, t.to_string(), x));
                true
            },
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
            Dedup {
                resume: ResumePoint::default(),
                covered: &mut Vec::new(),
            },
            &samples,
            &mut |_| Ok("t".to_string()),
            &mut |_, _, _, x| {
                excluded.push(x);
                true
            },
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
            Dedup {
                resume: ResumePoint::default(),
                covered: &mut Vec::new(),
            },
            &samples,
            &mut |_| {
                calls += 1;
                if calls == 2 {
                    Err(SttError::ModelNotReady)
                } else {
                    Ok("t".to_string())
                }
            },
            &mut |_, _, _, _| true,
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
            Dedup {
                resume: ResumePoint::default(),
                covered: &mut Vec::new(),
            },
            &samples,
            &mut |_| {
                calls += 1;
                if calls == 2 {
                    Err(SttError::Provider("engine".to_string()))
                } else {
                    Ok("t".to_string())
                }
            },
            &mut |s, _, _, _| {
                inserted.push(s);
                true
            },
            &|_, _| false,
        );
        assert_eq!(pass, BlockPass::Failed);
        assert_eq!(inserted, vec![10_000]);

        // Resume at 1 → only the remaining utterance runs.
        let mut resumed = Vec::new();
        process_utterances(
            &block(Track::Mic),
            &two_utterances(),
            Dedup {
                resume: ResumePoint { utterance: 1 },
                covered: &mut Vec::new(),
            },
            &samples,
            &mut |_| Ok("t2".to_string()),
            &mut |s, _, _, _| {
                resumed.push(s);
                true
            },
            &|_, _| false,
        );
        assert_eq!(resumed, vec![12_000]);
    }

    #[test]
    fn pass_skips_utterances_covered_by_live_chunks() {
        // A live chunk already persisted everything up to 11.5 s — the
        // sealing block must not re-transcribe or re-insert that span,
        // while a boundary-spanning utterance still runs in full.
        let samples = vec![0.0f32; 960_000];
        let mut calls = 0usize;
        let mut inserted = Vec::new();
        let pass = process_utterances(
            &block(Track::Mic),
            &two_utterances(),
            Dedup {
                resume: ResumePoint { utterance: 0 },
                // block offset 10_000 + 1.5 s already covered by a live chunk
                covered: &mut vec![(10_000, 11_500)],
            },
            &samples,
            &mut |_| {
                calls += 1;
                Ok("t".to_string())
            },
            &mut |s, e, t, _| {
                inserted.push((s, e, t.to_string()));
                true
            },
            &|_, _| false,
        );
        assert_eq!(pass, BlockPass::Done);
        assert_eq!(calls, 1, "only the uncovered utterance is transcribed");
        assert_eq!(inserted, vec![(12_000, 13_000, "t".to_string())]);
    }

    #[test]
    fn pass_trims_an_utterance_straddling_the_watermark() {
        // Chunk and block VAD split differently: a block utterance can start
        // before `covered_until` and end after it. The already-persisted head
        // is trimmed — the row and the transcribed samples keep only the
        // uncovered tail.
        let samples = vec![0.0f32; 960_000];
        let mut inserted = Vec::new();
        let mut transcribed_lens = Vec::new();
        let pass = process_utterances(
            &block(Track::Mic),
            // One utterance [0, 3 000) ms spanning the 1 500 ms watermark.
            &[Utterance {
                start_sample: 0,
                end_sample: 48_000,
                start_ms: 0,
                end_ms: 3_000,
            }],
            Dedup {
                resume: ResumePoint { utterance: 0 },
                covered: &mut vec![(10_000, 11_500)], // block offset 10_000 + 1.5 s
            },
            &samples,
            &mut |slice| {
                transcribed_lens.push(slice.len());
                Ok("t".to_string())
            },
            &mut |s, e, t, _| {
                inserted.push((s, e, t.to_string()));
                true
            },
            &|_, _| false,
        );
        assert_eq!(pass, BlockPass::Done);
        assert_eq!(
            inserted,
            vec![(11_500, 13_000, "t".to_string())],
            "the row starts at the watermark, not the utterance start"
        );
        assert_eq!(
            transcribed_lens,
            vec![24_000],
            "only the uncovered 1.5 s of samples is transcribed"
        );
    }

    #[test]
    fn pass_recovers_the_hole_left_by_a_dropped_chunk() {
        // Chunks before and after this utterance persisted their spans,
        // but the chunk covering its middle was dropped. The sealing
        // block must transcribe only the uncovered middle — a flat
        // watermark would skip the utterance (it ends before the later
        // chunk's coverage) and lose that speech permanently.
        let samples = vec![0.0f32; 960_000];
        let mut inserted = Vec::new();
        let mut transcribed_lens = Vec::new();
        let pass = process_utterances(
            &block(Track::Mic),
            &[Utterance {
                start_sample: 0,
                end_sample: 48_000,
                start_ms: 0,
                end_ms: 3_000,
            }],
            Dedup {
                resume: ResumePoint::default(),
                // covered: [10_000, 11_000) and [12_000, 13_000) — a hole
                // where the dropped chunk should have been.
                covered: &mut vec![(10_000, 11_000), (12_000, 13_000)],
            },
            &samples,
            &mut |slice| {
                transcribed_lens.push(slice.len());
                Ok("t".to_string())
            },
            &mut |s, e, t, _| {
                inserted.push((s, e, t.to_string()));
                true
            },
            &|_, _| false,
        );
        assert_eq!(pass, BlockPass::Done);
        assert_eq!(
            inserted,
            vec![(11_000, 12_000, "t".to_string())],
            "only the hole gets a new row"
        );
        assert_eq!(
            transcribed_lens,
            vec![16_000],
            "only the uncovered 1 s of samples is transcribed"
        );
    }

    #[test]
    fn coverage_merges_and_uncovered_splits() {
        let mut covered = Vec::new();
        record_coverage(&mut covered, 10, 20);
        record_coverage(&mut covered, 30, 40);
        // A bridging range merges both into one interval.
        record_coverage(&mut covered, 18, 32);
        assert_eq!(covered, vec![(10, 40)]);

        assert_eq!(uncovered(&covered, 0, 50), vec![(0, 10), (40, 50)]);
        assert!(uncovered(&covered, 15, 35).is_empty());
        assert_eq!(uncovered(&covered, 5, 15), vec![(5, 10)]);
        // Out-of-order inserts stay sorted/merged.
        record_coverage(&mut covered, 0, 5);
        assert_eq!(covered, vec![(0, 5), (10, 40)]);
    }

    #[test]
    fn busy_retry_skips_portions_already_inserted() {
        // First pass: the head portion inserts, then the tail hits
        // ModelNotReady. The retried pass must not re-insert the head —
        // recorded coverage drops it from `uncovered`.
        let samples = vec![0.0f32; 960_000];
        let utterances = vec![Utterance {
            start_sample: 0,
            end_sample: 48_000,
            start_ms: 0,
            end_ms: 3_000,
        }];
        let mut covered = vec![(10_000, 11_000), (12_000, 12_500)];

        let mut inserted = Vec::new();
        let mut calls = 0usize;
        let pass = process_utterances(
            &block(Track::Mic),
            &utterances,
            Dedup {
                resume: ResumePoint::default(),
                covered: &mut covered,
            },
            &samples,
            &mut |_| {
                calls += 1;
                if calls == 2 {
                    Err(SttError::ModelNotReady)
                } else {
                    Ok("t".to_string())
                }
            },
            &mut |s, e, t, _| {
                inserted.push((s, e, t.to_string()));
                true
            },
            &|_, _| false,
        );
        assert_eq!(pass, BlockPass::Busy(0));
        assert_eq!(inserted, vec![(11_000, 12_000, "t".to_string())]);

        // Retry resumes at utterance 0; only the still-uncovered tail runs.
        let pass = process_utterances(
            &block(Track::Mic),
            &utterances,
            Dedup {
                resume: ResumePoint { utterance: 0 },
                covered: &mut covered,
            },
            &samples,
            &mut |_| Ok("t".to_string()),
            &mut |s, e, t, _| {
                inserted.push((s, e, t.to_string()));
                true
            },
            &|_, _| false,
        );
        assert_eq!(pass, BlockPass::Done);
        assert_eq!(
            inserted,
            vec![
                (11_000, 12_000, "t".to_string()),
                (12_500, 13_000, "t".to_string())
            ],
            "the retried pass adds only the uncovered tail"
        );
    }

    #[test]
    fn pass_empty_text_inserts_nothing() {
        let samples = vec![0.0f32; 960_000];
        let mut inserted = 0usize;
        let mut covered = Vec::new();
        let pass = process_utterances(
            &block(Track::Mic),
            &two_utterances(),
            Dedup {
                resume: ResumePoint::default(),
                covered: &mut covered,
            },
            &samples,
            &mut |_| Ok("   ".to_string()),
            &mut |_, _, _, _| {
                inserted += 1;
                true
            },
            &|_, _| false,
        );
        assert_eq!(pass, BlockPass::Done);
        assert_eq!(inserted, 0);
        // Empty transcripts still settle the range — a busy retry or the
        // sealing block must not re-run STT on silence.
        assert_eq!(covered, vec![(10_000, 11_000), (12_000, 13_000)]);
    }

    #[test]
    fn sliver_portions_are_settled_without_calling_stt() {
        // Boundary rounding between chunk coverage and a block's utterance
        // can leave a 1 ms sliver — transcribing it could fail the whole
        // block on a provider hiccup over 16 samples.
        let samples = vec![0.0f32; 960_000];
        let mut calls = 0usize;
        let mut covered = vec![(10_000, 11_000), (11_001, 13_000)];
        let pass = process_utterances(
            &block(Track::Mic),
            &[Utterance {
                start_sample: 0,
                end_sample: 48_000,
                start_ms: 0,
                end_ms: 3_000,
            }],
            Dedup {
                resume: ResumePoint::default(),
                covered: &mut covered,
            },
            &samples,
            &mut |_| {
                calls += 1;
                Ok("t".to_string())
            },
            &mut |_, _, _, _| true,
            &|_, _| false,
        );
        assert_eq!(pass, BlockPass::Done);
        assert_eq!(calls, 0, "the 1 ms sliver never reaches the engine");
        assert_eq!(
            covered,
            vec![(10_000, 13_000)],
            "the sliver merges into the coverage set"
        );
    }

    #[test]
    fn a_failed_insert_leaves_the_portion_uncovered_and_fails_the_pass() {
        // If the segment row did not land, marking the range covered would
        // lose the text permanently — the pass fails instead and the block
        // stays pending for post-processing.
        let samples = vec![0.0f32; 960_000];
        let mut covered = Vec::new();
        let pass = process_utterances(
            &block(Track::Mic),
            &two_utterances(),
            Dedup {
                resume: ResumePoint::default(),
                covered: &mut covered,
            },
            &samples,
            &mut |_| Ok("t".to_string()),
            &mut |_, _, _, _| false,
            &|_, _| false,
        );
        assert_eq!(pass, BlockPass::Failed);
        assert!(
            uncovered(&covered, 10_000, 11_000) == vec![(10_000, 11_000)],
            "nothing was persisted, so nothing may be marked covered"
        );
    }

    #[test]
    fn drop_open_tail_pops_an_utterance_reaching_the_buffer_end() {
        // The chunk ends mid-speech: the open tail must not persist — the
        // sealing block re-covers it — and only the samples before it
        // count as covered.
        let mut utterances = vec![
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
        ];
        let covered = drop_open_tail(&mut utterances, true, 48_000);
        assert_eq!(covered, 32_000);
        assert_eq!(utterances.len(), 1);
        assert_eq!(utterances[0].end_sample, 16_000);
    }

    #[test]
    fn drop_open_tail_tolerates_a_frame_aligned_end_past_the_buffer() {
        // Chunk lengths need not be frame multiples — a segmenter that
        // reports the padded frame edge (end_sample > len) must still
        // have its tail dropped. `==` would miss it.
        let mut utterances = vec![Utterance {
            start_sample: 32_000,
            end_sample: 48_160, // 48 000-sample buffer, padded tail
            start_ms: 2_000,
            end_ms: 3_000,
        }];
        let covered = drop_open_tail(&mut utterances, true, 48_000);
        assert_eq!(covered, 32_000);
        assert!(utterances.is_empty());
    }

    #[test]
    fn drop_open_tail_keeps_a_closed_tail() {
        // `tail_voiced` false means the last frame was silence — the
        // final utterance closed on its own and must persist.
        let mut utterances = vec![Utterance {
            start_sample: 32_000,
            end_sample: 48_000,
            start_ms: 2_000,
            end_ms: 3_000,
        }];
        let covered = drop_open_tail(&mut utterances, false, 48_000);
        assert_eq!(covered, 48_000);
        assert_eq!(utterances.len(), 1);
    }

    #[test]
    fn drop_open_tail_keeps_an_utterance_ending_before_the_end() {
        // Voiced last frame but the last utterance closed earlier
        // (trailing blip < min_frames): nothing to drop.
        let mut utterances = vec![Utterance {
            start_sample: 0,
            end_sample: 16_000,
            start_ms: 0,
            end_ms: 1_000,
        }];
        let covered = drop_open_tail(&mut utterances, true, 48_000);
        assert_eq!(covered, 48_000);
        assert_eq!(utterances.len(), 1);
    }
}
