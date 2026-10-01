//! Dictation-coexistence bookkeeping (FR-009-10, AC-009-03).
//!
//! The session worker opens/closes dictation-capture intervals on the
//! meeting clock as `session://state` transitions arrive; the transcriber
//! thread consults the shared tracker to flag overlapping `mic` utterances
//! as `excluded`, and the worker writes one `dictation_marker` per closed
//! interval. One interval per dictation session — a dictation spanning a
//! block boundary is still a single marker.

use std::sync::{Arc, Mutex};

/// Whether a `session://state` `state` value means the dictation session is
/// capturing microphone audio (the FR-009-10 interval signal).
pub(crate) fn dictation_capturing(state: &str) -> bool {
    matches!(state, "arming" | "recording")
}

/// `dictation_marker` text — the spec-visible label is "Ditado" (AC-009-03).
pub(crate) fn dictation_marker_text(lang: &str) -> &'static str {
    if lang == "pt-BR" {
        "Ditado"
    } else {
        "Dictation"
    }
}

/// `[start_ms, end_ms)` dictation-capture intervals on the meeting clock.
#[derive(Debug, Default)]
pub struct DictationTracker {
    /// Closed spans, in the order they closed.
    closed: Vec<(i64, i64)>,
    /// Open span start while a dictation session is capturing.
    open_since: Option<i64>,
}

/// Shared between the session worker (writer) and the transcriber thread.
pub(crate) type SharedDictationTracker = Arc<Mutex<DictationTracker>>;

impl DictationTracker {
    /// A dictation started capturing at `at_ms`. Repeated `capturing`
    /// transitions while one is open are ignored — the first open wins.
    /// Returns `true` when a new interval opened.
    pub fn begin(&mut self, at_ms: i64) -> bool {
        if self.open_since.is_some() {
            return false;
        }
        self.open_since = Some(at_ms);
        true
    }

    /// Dictation stopped capturing at `at_ms`; returns the closed interval
    /// when it is non-empty (`None` without an open interval or for
    /// zero-length spans).
    pub fn end(&mut self, at_ms: i64) -> Option<(i64, i64)> {
        let start = self.open_since.take()?;
        let end = at_ms.max(start);
        (end > start).then(|| {
            self.closed.push((start, end));
            (start, end)
        })
    }

    /// Whether `[start_ms, end_ms)` overlaps any dictation interval; the
    /// open one counts to +∞.
    pub fn overlaps(&self, start_ms: i64, end_ms: i64) -> bool {
        if end_ms <= start_ms {
            return false;
        }
        self.closed.iter().any(|&(a, b)| start_ms < b && end_ms > a)
            || self.open_since.is_some_and(|a| end_ms > a)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dictation_interval_excludes_overlapping_ranges() {
        let mut tracker = DictationTracker::default();
        tracker.begin(1_000);
        // While open, everything that reaches into it is excluded.
        assert!(tracker.overlaps(500, 1_500));
        assert!(tracker.overlaps(1_000, 1_001));
        assert!(
            !tracker.overlaps(0, 1_000),
            "ends exactly at open → no overlap"
        );
        assert!(
            !tracker.overlaps(2_000, 2_000),
            "empty range never overlaps"
        );

        assert_eq!(tracker.end(5_000), Some((1_000, 5_000)));
        assert!(tracker.overlaps(4_999, 5_500));
        assert!(
            !tracker.overlaps(5_000, 9_000),
            "start < b fails at the edge"
        );
        assert!(tracker.overlaps(999, 1_001));
        assert!(!tracker.overlaps(0, 1_000));
    }

    #[test]
    fn dictation_begin_is_idempotent_and_end_requires_open() {
        let mut tracker = DictationTracker::default();
        assert_eq!(tracker.end(100), None);
        assert!(tracker.begin(1_000));
        assert!(!tracker.begin(2_000), "nested opens don't shift the start");
        assert_eq!(tracker.end(5_000), Some((1_000, 5_000)));
        // Zero-length span produces no interval/marker.
        assert!(tracker.begin(9_000));
        assert_eq!(tracker.end(9_000), None);
    }

    #[test]
    fn dictation_spanning_blocks_is_one_interval() {
        // AC-009-03: a dictation crossing a block boundary is one marker —
        // the interval lives on the meeting clock, not per block.
        let mut tracker = DictationTracker::default();
        tracker.begin(55_000);
        tracker.end(65_000);
        assert!(tracker.overlaps(59_000, 61_000));
        assert!(tracker.overlaps(50_000, 70_000));
    }

    #[test]
    fn dictation_state_transitions_map_to_capturing() {
        for state in ["arming", "recording"] {
            assert!(dictation_capturing(state), "{state} is capture");
        }
        for state in [
            "idle",
            "transcribing",
            "processing",
            "inserting",
            "done",
            "error",
        ] {
            assert!(!dictation_capturing(state), "{state} ends capture");
        }
    }

    #[test]
    fn dictation_marker_text_is_localized() {
        assert_eq!(dictation_marker_text("pt-BR"), "Ditado");
        assert_eq!(dictation_marker_text("en"), "Dictation");
    }
}
