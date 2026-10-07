//! Pure coverage math for step 1 ("transcribe pending blocks").
//!
//! The question step 1 answers: *which sealed WAV blocks have no `speech`
//! segment yet?* Getting it right is what makes both the original pass and
//! `meeting_retry_processing` idempotent — a block that already produced
//! segments is never retranscribed, and a block that failed mid-pass is
//! revisited on the next run.
//!
//! Two inputs make the answer exact:
//!
//! - `meeting_blocks` rows (migration 11) — the session worker records each
//!   sealed block's `start_offset_ms`/`duration_ms`, so a `<track>-NNNN.wav`
//!   maps to the meeting clock without guessing. A `meeting_blocks` row that
//!   never got written (crash between fsync and the row upsert) falls back
//!   to [`resolve_spans`]' chaining: contiguous inside a capture run, jumped
//!   past `gap_marker`s that open where the chain lands.
//! - `meeting_segments` rows (`kind='speech'`) — the coverage a block needs
//!   to escape the pending set. `gap_marker`/`dictation_marker` rows never
//!   count as coverage.

use std::collections::HashMap;

use crate::db::meetings::MeetingSegment;

/// A sealed block's meeting-clock coverage `[start_ms, end_ms)`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BlockSpan {
    pub start_ms: i64,
    pub end_ms: i64,
}

/// How far the chaining fallback may drift from a `gap_marker`'s recorded
/// start before we stop snapping to it — decoded durations sum exactly, but
/// markers are stamped from `Instant` deltas and can sit a few hundred ms
/// off a clean block boundary.
const GAP_SNAP_MS: i64 = 1_000;

/// Compute the meeting-clock span of every scanned block, in input order.
///
/// - `blocks`: `(index, duration_ms)` in ascending index order (the
///   `scan_meeting_blocks` ordering for one track).
/// - `placements`: `(index, start_ms, end_ms)` from `meeting_blocks` rows —
///   authoritative when present.
/// - `gaps`: `(start_ms, end_ms)` `gap_marker`s applicable to this track,
///   any order.
pub fn resolve_spans(
    blocks: &[(u32, i64)],
    placements: &[(u32, i64, i64)],
    gaps: &[(i64, i64)],
) -> Vec<BlockSpan> {
    let placed: HashMap<u32, (i64, i64)> = placements
        .iter()
        .map(|(index, start, end)| (*index, (*start, *end)))
        .collect();
    let mut gaps: Vec<(i64, i64)> = gaps.to_vec();
    gaps.sort_unstable_by_key(|(start, _)| *start);
    let mut next_gap = 0usize;

    // Expected start of the next *unplaced* block: the meeting-clock point
    // where the previous block's audio ended, plus any gap markers that open
    // there (a pause ends one run and the resumed capture's blocks start at
    // the marker's end).
    let mut cursor = 0i64;
    blocks
        .iter()
        .map(|(index, duration_ms)| {
            let span = match placed.get(index) {
                Some(&(start_ms, end_ms)) => {
                    cursor = end_ms.max(cursor);
                    BlockSpan { start_ms, end_ms }
                }
                None => {
                    while next_gap < gaps.len() {
                        let (gap_start, gap_end) = gaps[next_gap];
                        if gap_end <= cursor {
                            next_gap += 1; // already behind us
                        } else if gap_start <= cursor + GAP_SNAP_MS {
                            // A pause gap at this boundary — audio resumes at
                            // its end.
                            cursor = cursor.max(gap_end);
                            next_gap += 1;
                        } else {
                            break;
                        }
                    }
                    let span = BlockSpan {
                        start_ms: cursor,
                        end_ms: cursor + (*duration_ms).max(0),
                    };
                    cursor = span.end_ms;
                    span
                }
            };
            span
        })
        .collect()
}

/// Ranges already covered by `speech` segments on `track`, as merged
/// `(start_ms, end_ms)` pairs sorted by start. `gap_marker`/`dictation_marker`
/// rows never count — they mark silence, not speech.
pub fn covered_ranges(segments: &[MeetingSegment], track: &str) -> Vec<(i64, i64)> {
    let mut ranges: Vec<(i64, i64)> = segments
        .iter()
        .filter(|s| s.track == track && s.kind == "speech" && s.end_ms > s.start_ms)
        .map(|s| (s.start_ms, s.end_ms))
        .collect();
    ranges.sort_unstable();
    // Merge overlapping/adjacent ranges so `pending_spans` overlap tests stay
    // linear.
    let mut merged: Vec<(i64, i64)> = Vec::with_capacity(ranges.len());
    for (start, end) in ranges {
        match merged.last_mut() {
            Some((_, last_end)) if start <= *last_end => *last_end = (*last_end).max(end),
            _ => merged.push((start, end)),
        }
    }
    merged
}

/// The subset of `spans` that still needs transcription: a block is pending
/// when no covered range **overlaps** it. Overlap (not containment) is the
/// right test — a partially transcribed block is not re-attempted: its
/// VAD regions run on the meeting clock, so a retry would re-insert the same
/// offsets; the audio already spoken for is kept.
pub fn pending_spans<'a>(spans: &'a [BlockSpan], covered: &[(i64, i64)]) -> Vec<&'a BlockSpan> {
    spans
        .iter()
        .filter(|span| {
            !covered
                .iter()
                .any(|(start, end)| span.start_ms < *end && *start < span.end_ms)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::meeting::blocks::Track;

    fn segment(track: &str, start: i64, end: i64, kind: &str) -> MeetingSegment {
        let mut s = MeetingSegment::new("m1", track, start, end, "texto");
        s.kind = kind.to_string();
        s
    }

    #[test]
    fn placements_win_over_chaining() {
        // Blocks 1 and 3 have recorded offsets (a pause sits between them);
        // block 2's row is missing — it chains after block 1.
        let spans = resolve_spans(
            &[(1, 60_000), (2, 60_000), (3, 30_000)],
            &[(1, 0, 60_000), (3, 720_000, 750_000)],
            &[],
        );
        assert_eq!(
            spans,
            vec![
                BlockSpan {
                    start_ms: 0,
                    end_ms: 60_000
                },
                BlockSpan {
                    start_ms: 60_000,
                    end_ms: 120_000
                },
                BlockSpan {
                    start_ms: 720_000,
                    end_ms: 750_000
                },
            ]
        );
    }

    #[test]
    fn chaining_snaps_across_gap_markers() {
        // No placements at all (a meeting recovered pre-T-067, hypothetically):
        // 2 blocks, a 5 min pause recorded on the clock, then 1 more block.
        let spans = resolve_spans(
            &[(1, 60_000), (2, 60_000), (3, 60_000)],
            &[],
            &[(120_000, 420_000)],
        );
        assert_eq!(
            spans,
            vec![
                BlockSpan {
                    start_ms: 0,
                    end_ms: 60_000
                },
                BlockSpan {
                    start_ms: 60_000,
                    end_ms: 120_000
                },
                // The chain lands at 120_000 — exactly where the marker opens —
                // so the block resumes at its end.
                BlockSpan {
                    start_ms: 420_000,
                    end_ms: 480_000
                },
            ]
        );
    }

    #[test]
    fn gap_snap_tolerates_marker_drift() {
        // Marker stamped 300 ms after the true boundary still snaps.
        let spans = resolve_spans(&[(1, 60_000), (2, 60_000)], &[], &[(60_300, 120_300)]);
        assert_eq!(spans[1].start_ms, 120_300);
    }

    #[test]
    fn distant_gaps_do_not_pull_the_chain() {
        // A gap far past the current cursor belongs to a later run — the
        // block still chains right after the previous one.
        let spans = resolve_spans(&[(1, 60_000), (2, 60_000)], &[], &[(999_000, 1_000_000)]);
        assert_eq!(spans[1].start_ms, 60_000);
    }

    #[test]
    fn covered_ranges_merge_and_skip_markers() {
        let segments = vec![
            segment("mic", 0, 5_000, "speech"),
            segment("mic", 4_000, 9_000, "speech"), // overlaps → merged
            segment("mic", 20_000, 30_000, "gap_marker"), // marker never covers
            segment("system", 0, 60_000, "speech"), // other track
            segment("mic", 15_000, 16_000, "speech"),
        ];
        assert_eq!(
            covered_ranges(&segments, "mic"),
            vec![(0, 9_000), (15_000, 16_000)]
        );
    }

    #[test]
    fn pending_is_the_uncovered_subset() {
        let spans = vec![
            BlockSpan {
                start_ms: 0,
                end_ms: 60_000,
            },
            BlockSpan {
                start_ms: 60_000,
                end_ms: 120_000,
            },
        ];
        // A live-transcription segment inside block 2 covers it — overlap,
        // not containment, is the test.
        let covered = vec![(65_000, 80_000)];
        let pending = pending_spans(&spans, &covered);
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].start_ms, 0);
        // Full coverage → nothing pending.
        let covered = vec![(0, 60_000), (60_000, 120_000)];
        assert!(pending_spans(&spans, &covered).is_empty());
    }

    #[test]
    fn track_label_maps_to_segment_track() {
        // The constant pipeline relies on: `Track::label()` is the
        // `meeting_segments.track` value.
        assert_eq!(Track::Mic.label(), "mic");
        assert_eq!(Track::System.label(), "system");
    }
}
