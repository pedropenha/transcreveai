//! Fair bounded per-track queues for the live transcriber.
//!
//! One FIFO per [`Track`] with a single consumer that alternates between
//! them — a stalled `system` track can never starve `mic` (and vice versa).
//! `push` rejects when the FIFO is full; the block's `meeting_blocks` row
//! simply stays `transcribed = 0` for T-067's post-processing, so nothing is
//! dropped silently.

use std::collections::VecDeque;

use crate::meeting::blocks::{SealedBlock, Track};

/// One sealed block queued for transcription.
pub(crate) struct BlockJob {
    pub block: SealedBlock,
    /// Utterances already persisted — resume point after a model-busy
    /// requeue so a retried block never inserts duplicate rows.
    pub resume_utterance: usize,
    /// Model-busy requeues consumed so far (bounded by `MAX_BUSY_RETRIES`).
    pub busy_retries: u32,
}

impl BlockJob {
    pub fn new(block: SealedBlock) -> Self {
        Self {
            block,
            resume_utterance: 0,
            busy_retries: 0,
        }
    }
}

fn slot(track: Track) -> usize {
    match track {
        Track::Mic => 0,
        Track::System => 1,
    }
}

fn other(track: Track) -> Track {
    match track {
        Track::Mic => Track::System,
        Track::System => Track::Mic,
    }
}

/// The transcriber thread's work backlog.
pub(crate) struct TrackQueues {
    queues: [VecDeque<BlockJob>; 2],
    capacity: usize,
    last_served: Option<Track>,
}

impl TrackQueues {
    pub fn new(capacity: usize) -> Self {
        Self {
            queues: [VecDeque::new(), VecDeque::new()],
            capacity,
            last_served: None,
        }
    }

    /// `Err(job)` when the track's FIFO is full.
    pub fn push(&mut self, job: BlockJob) -> std::result::Result<(), BlockJob> {
        let queue = &mut self.queues[slot(job.block.track)];
        if queue.len() >= self.capacity {
            return Err(job);
        }
        queue.push_back(job);
        Ok(())
    }

    /// Put a partially processed block back at the front of its track —
    /// `resume_utterance` preserves progress (no duplicate rows). Ignores the
    /// capacity check: the job already held a slot.
    pub fn requeue_front(&mut self, job: BlockJob) {
        self.queues[slot(job.block.track)].push_front(job);
    }

    /// Oldest queued block, alternating tracks when both have work.
    pub fn pop(&mut self) -> Option<BlockJob> {
        let prefer = match self.last_served {
            Some(Track::Mic) => Track::System,
            _ => Track::Mic,
        };
        for track in [prefer, other(prefer)] {
            if let Some(job) = self.queues[slot(track)].pop_front() {
                self.last_served = Some(track);
                return Some(job);
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::meeting::blocks::block_filename;
    use std::path::PathBuf;

    fn job(track: Track, index: u32) -> BlockJob {
        BlockJob::new(SealedBlock {
            track,
            index,
            path: PathBuf::from(block_filename(track, index)),
            start_offset_ms: index as u64 * 60_000,
            duration_ms: 60_000,
            samples: 960_000,
        })
    }

    #[test]
    fn queues_alternate_tracks_fairly() {
        let mut queues = TrackQueues::new(4);
        let jobs = [
            job(Track::Mic, 1),
            job(Track::Mic, 2),
            job(Track::System, 1),
            job(Track::System, 2),
        ];
        for job in jobs {
            queues.push(job).map_err(|_| "queue unexpectedly full").ok();
        }

        let order: Vec<(Track, u32)> = (0..4)
            .filter_map(|_| queues.pop())
            .map(|j| (j.block.track, j.block.index))
            .collect();
        assert_eq!(
            order,
            vec![
                (Track::Mic, 1),
                (Track::System, 1),
                (Track::Mic, 2),
                (Track::System, 2)
            ],
            "alternating tracks — neither starves"
        );
        assert!(queues.pop().is_none());
    }

    #[test]
    fn queue_full_rejects_instead_of_dropping() {
        let mut queues = TrackQueues::new(2);
        assert!(queues.push(job(Track::Mic, 1)).is_ok());
        assert!(queues.push(job(Track::Mic, 2)).is_ok());
        let rejected = queues.push(job(Track::Mic, 3));
        assert!(rejected.is_err(), "bounded FIFO refuses the third block");
        // The other track still has room — per-track bound.
        assert!(queues.push(job(Track::System, 1)).is_ok());
    }

    #[test]
    fn requeue_front_preserves_resume_progress() {
        let mut queues = TrackQueues::new(4);
        assert!(queues.push(job(Track::Mic, 1)).is_ok());
        assert!(queues.push(job(Track::Mic, 2)).is_ok());

        let mut first = queues.pop().unwrap_or_else(|| unreachable!());
        first.resume_utterance = 3;
        queues.requeue_front(first);

        let again = queues.pop().unwrap_or_else(|| unreachable!());
        assert_eq!((again.block.track, again.block.index), (Track::Mic, 1));
        assert_eq!(again.resume_utterance, 3);
    }
}
