//! Incremental on-disk recording for a meeting (FR-009-05, AC-009-04).
//!
//! Each track (`mic`, `system`) is written as a sequence of 60 s WAV blocks
//! (`<track>-NNNN.wav`, 16 kHz mono 16-bit) under `audio/meetings/<id>/`.
//! Every sealed block is fsync'd before the writer moves on, so a crash loses
//! at most the in-flight block (NFR-009-03) and leaves only complete files.
//!
//! [`BlockWriter`] is deliberately free of threads and channels — it is the
//! pure, testable rotation core; [`super::capture`] drives it from frame
//! callbacks on its own writer threads.

use std::{
    fs::{self, File},
    io::BufWriter,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use anyhow::{Context, Result};
use hound::{WavSpec, WavWriter};

use crate::audio_toolkit::constants::WHISPER_SAMPLE_RATE;

/// FR-009-05: one block per minute per track.
pub const BLOCK_DURATION: Duration = Duration::from_secs(60);

/// The two independent capture tracks (FR-009-03). `label()` matches the
/// `meeting_segments.track` CHECK constraint in `data-model.md` §2.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Track {
    Mic,
    System,
}

impl Track {
    pub const ALL: [Track; 2] = [Track::Mic, Track::System];

    /// `'mic' | 'system'` — file prefix and `meeting_segments.track` value.
    pub fn label(self) -> &'static str {
        match self {
            Track::Mic => "mic",
            Track::System => "system",
        }
    }
}

/// Block file name for `track` + 1-based `index` (`mic-0001.wav`, …).
/// Four digits keep lexicographic order == chronological order for meetings
/// up to ~7 days long.
pub fn block_filename(track: Track, index: u32) -> String {
    format!("{}-{index:04}.wav", track.label())
}

/// Inverse of [`block_filename`]; `None` for anything that is not a block
/// file (stray files are ignored by scans, never mistaken for audio).
pub fn parse_block_filename(name: &str) -> Option<(Track, u32)> {
    let (stem, ext) = name.rsplit_once('.')?;
    if !ext.eq_ignore_ascii_case("wav") {
        return None;
    }
    let (prefix, digits) = stem.split_once('-')?;
    let track = match prefix {
        "mic" => Track::Mic,
        "system" => Track::System,
        _ => return None,
    };
    let index: u32 = digits.parse().ok()?;
    (index >= 1).then_some((track, index))
}

/// `audio/meetings/` under the app data dir (`data-model.md` §1).
pub fn meetings_root(app_data_dir: &Path) -> PathBuf {
    app_data_dir.join("audio").join("meetings")
}

/// Directory holding one meeting's blocks (`meetings/<id>/` — `id` is a
/// uuid we generated, so a plain join is safe).
pub fn meeting_audio_dir(app_data_dir: &Path, meeting_id: &str) -> PathBuf {
    meetings_root(app_data_dir).join(meeting_id)
}

/// One completed block file on disk.
#[derive(Clone, Debug, PartialEq)]
pub struct SealedBlock {
    pub track: Track,
    /// 1-based sequence within the track.
    pub index: u32,
    pub path: PathBuf,
    /// Offset of the block's first frame from capture start, from a shared
    /// monotonic clock — comparable across both tracks (FR-009-03).
    pub start_offset_ms: u64,
    pub duration_ms: u64,
    /// Samples written (at [`WHISPER_SAMPLE_RATE`]).
    pub samples: usize,
}

/// Rotating writer for a single track. Buffers samples until a full
/// `block_samples` chunk is pending, then seals it to `<track>-NNNN.wav`
/// (write + fsync) and starts the next. The in-flight tail is flushed by
/// [`Self::finish`].
pub struct BlockWriter {
    dir: PathBuf,
    track: Track,
    block_samples: usize,
    /// Monotonic base shared by both tracks.
    t0: Instant,
    /// When the current block's first frame arrived.
    pending_start: Option<Instant>,
    pending: Vec<f32>,
    next_index: u32,
    sealed: Vec<SealedBlock>,
}

impl BlockWriter {
    /// Create the track's directory and start a writer with the 60 s spec
    /// block size.
    pub fn new(dir: &Path, track: Track, t0: Instant) -> Result<Self> {
        Self::with_block_samples(
            dir,
            track,
            t0,
            BLOCK_DURATION.as_secs() as usize * WHISPER_SAMPLE_RATE as usize,
        )
    }

    /// Same as [`Self::new`] with an explicit block size — tests rotate
    /// without waiting a minute.
    pub fn with_block_samples(
        dir: &Path,
        track: Track,
        t0: Instant,
        block_samples: usize,
    ) -> Result<Self> {
        fs::create_dir_all(dir).with_context(|| format!("Failed to create {}", dir.display()))?;
        Ok(Self {
            dir: dir.to_path_buf(),
            track,
            block_samples,
            t0,
            pending_start: None,
            pending: Vec::with_capacity(block_samples),
            next_index: 1,
            sealed: Vec::new(),
        })
    }

    /// Blocks sealed so far, in order.
    pub fn sealed(&self) -> &[SealedBlock] {
        &self.sealed
    }

    /// Buffer a frame; seal and fsync every block it completes. `now` stamps
    /// block boundaries so `start_offset_ms` reflects real elapsed time —
    /// including capture gaps (device swaps produce no samples).
    pub fn push(&mut self, frame: &[f32], now: Instant) -> Result<()> {
        if frame.is_empty() {
            return Ok(());
        }
        if self.pending_start.is_none() {
            self.pending_start = Some(now);
        }
        self.pending.extend_from_slice(frame);
        while self.pending.len() >= self.block_samples {
            self.seal(self.block_samples, now)?;
        }
        Ok(())
    }

    /// Seal the in-flight tail as a final short block, if any. The writer
    /// stays usable for [`Self::into_sealed`] even when sealing fails.
    pub fn drain_tail(&mut self, now: Instant) -> Result<()> {
        while !self.pending.is_empty() {
            let count = self.pending.len().min(self.block_samples);
            self.seal(count, now)?;
        }
        Ok(())
    }

    /// Consume the writer and return every sealed block.
    pub fn into_sealed(self) -> Vec<SealedBlock> {
        self.sealed
    }

    /// Flush the in-flight tail (if any) as a final short block and hand back
    /// everything sealed.
    pub fn finish(mut self, now: Instant) -> Result<Vec<SealedBlock>> {
        self.drain_tail(now)?;
        Ok(self.sealed)
    }

    /// Write the first `count` pending samples as the next block file.
    fn seal(&mut self, count: usize, boundary: Instant) -> Result<()> {
        let index = self.next_index;
        let path = self.dir.join(block_filename(self.track, index));
        write_wav_block(&path, &self.pending[..count])?;

        let start_offset_ms = self
            .pending_start
            .map(|start| start.duration_since(self.t0).as_millis() as u64)
            .unwrap_or(0);
        self.sealed.push(SealedBlock {
            track: self.track,
            index,
            path,
            start_offset_ms,
            duration_ms: (count as u64 * 1_000) / WHISPER_SAMPLE_RATE as u64,
            samples: count,
        });

        self.pending.drain(..count);
        self.next_index += 1;
        // The next block's first frame is whatever came after this boundary.
        self.pending_start = (!self.pending.is_empty()).then_some(boundary);
        Ok(())
    }
}

/// Write `samples` as a 16 kHz mono 16-bit WAV and fsync it (FR-009-05):
/// once `seal` returns, the block survives a process crash.
fn write_wav_block(path: &Path, samples: &[f32]) -> Result<()> {
    const SPEC: WavSpec = WavSpec {
        channels: 1,
        sample_rate: WHISPER_SAMPLE_RATE,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };

    let file =
        File::create(path).with_context(|| format!("Failed to create block {}", path.display()))?;
    // `&File` keeps the handle so we can fsync after finalize flushes the
    // buffered writer and rewrites the header.
    let mut writer = WavWriter::new(BufWriter::new(&file), SPEC)
        .with_context(|| format!("Failed to open block {} for writing", path.display()))?;
    for sample in samples {
        writer
            .write_sample(
                (*sample * i16::MAX as f32).clamp(i16::MIN as f32, i16::MAX as f32) as i16,
            )
            .with_context(|| format!("Failed to write block {}", path.display()))?;
    }
    writer
        .finalize()
        .with_context(|| format!("Failed to finalize block {}", path.display()))?;
    file.sync_all()
        .with_context(|| format!("Failed to fsync block {}", path.display()))?;
    Ok(())
}

/// One block file found by [`scan_meeting_blocks`], with its decoded length.
#[derive(Clone, Debug, PartialEq)]
pub struct ScannedBlock {
    pub track: Track,
    pub index: u32,
    pub path: PathBuf,
    /// Decoded frames in the file (16 kHz → `sample_count / 16` ms).
    pub sample_count: usize,
}

/// Result of scanning a meeting's audio directory after a crash.
#[derive(Debug, Default)]
pub struct MeetingBlocks {
    /// Readable blocks, ordered by track then index.
    pub blocks: Vec<ScannedBlock>,
    /// Files that match the block pattern but fail to decode — typically the
    /// in-flight block truncated by the crash. Callers skip them; everything
    /// before them is intact.
    pub corrupt: Vec<PathBuf>,
}

/// Enumerate a meeting's block files — the recovery half of AC-009-04: given
/// the directory of a meeting left in `recording` by a dead process, this
/// returns every intact block for processing.
///
/// Missing directory is `Ok` with no blocks (a meeting may have crashed
/// before its first block sealed).
pub fn scan_meeting_blocks(dir: &Path) -> Result<MeetingBlocks> {
    let mut found = MeetingBlocks::default();
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(found),
        Err(err) => return Err(err).with_context(|| format!("Failed to read {}", dir.display())),
    };

    for entry in entries {
        let entry = entry?;
        let file_name = entry.file_name();
        let Some(name) = file_name.to_str() else {
            continue;
        };
        let Some((track, index)) = parse_block_filename(name) else {
            continue;
        };
        let path = entry.path();
        match hound::WavReader::open(&path) {
            Ok(reader) => found.blocks.push(ScannedBlock {
                track,
                index,
                path,
                sample_count: reader.len() as usize,
            }),
            Err(_) => found.corrupt.push(path),
        }
    }

    found
        .blocks
        .sort_by_key(|block| (block.track.label(), block.index));
    found.corrupt.sort();
    Ok(found)
}

#[allow(clippy::unwrap_used, clippy::panic)]
#[cfg(test)]
mod tests {
    use super::*;

    fn frames(count: usize, value: f32) -> Vec<f32> {
        vec![value; count]
    }

    #[test]
    fn block_filename_roundtrips() {
        assert_eq!(block_filename(Track::Mic, 1), "mic-0001.wav");
        assert_eq!(block_filename(Track::System, 42), "system-0042.wav");
        assert_eq!(parse_block_filename("mic-0001.wav"), Some((Track::Mic, 1)));
        assert_eq!(
            parse_block_filename("system-0042.wav"),
            Some((Track::System, 42))
        );
        assert_eq!(parse_block_filename("sys-0001.wav"), None);
        assert_eq!(parse_block_filename("mic-0000.wav"), None);
        assert_eq!(parse_block_filename("mic-0001.mp3"), None);
        assert_eq!(parse_block_filename("README"), None);
        assert_eq!(parse_block_filename("mic-.wav"), None);
    }

    #[test]
    fn writer_rotates_at_the_block_boundary() {
        let dir = tempfile::tempdir().unwrap();
        let t0 = Instant::now();
        // 100-sample blocks: 2.5 blocks of input seal 2 and leave a tail.
        let mut writer = BlockWriter::with_block_samples(dir.path(), Track::Mic, t0, 100).unwrap();

        writer
            .push(&frames(240, 0.5), t0 + Duration::from_millis(10))
            .unwrap();

        assert_eq!(writer.sealed().len(), 2);
        assert_eq!(writer.sealed()[0].index, 1);
        assert_eq!(writer.sealed()[0].samples, 100);
        assert_eq!(writer.sealed()[0].start_offset_ms, 10);
        // The second block starts where the first ended, not at first push.
        assert_eq!(writer.sealed()[1].index, 2);
        assert!(dir.path().join("mic-0001.wav").exists());
        assert!(dir.path().join("mic-0002.wav").exists());
        assert!(!dir.path().join("mic-0003.wav").exists());

        // Sealed files decode and hold exactly their samples.
        crate::audio_toolkit::verify_wav_file(&writer.sealed()[0].path, 100).unwrap();
        crate::audio_toolkit::verify_wav_file(&writer.sealed()[1].path, 100).unwrap();

        let sealed = writer.finish(t0 + Duration::from_millis(20)).unwrap();
        assert_eq!(sealed.len(), 3);
        assert_eq!(sealed[2].samples, 40);
        crate::audio_toolkit::verify_wav_file(&sealed[2].path, 40).unwrap();
    }

    #[test]
    fn finish_without_audio_seals_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let writer = BlockWriter::new(dir.path(), Track::System, Instant::now()).unwrap();
        assert!(writer.finish(Instant::now()).unwrap().is_empty());
    }

    #[test]
    fn scan_lists_blocks_by_track_and_marks_corrupt() {
        let dir = tempfile::tempdir().unwrap();
        let t0 = Instant::now();
        let mut mic = BlockWriter::with_block_samples(dir.path(), Track::Mic, t0, 10).unwrap();
        let mut sys = BlockWriter::with_block_samples(dir.path(), Track::System, t0, 10).unwrap();
        mic.push(&frames(25, 0.1), t0).unwrap();
        sys.push(&frames(10, 0.2), t0).unwrap();
        mic.finish(t0).unwrap();
        sys.finish(t0).unwrap();

        // A truncated in-flight block, as a crash would leave it.
        fs::write(dir.path().join("mic-0004.wav"), b"RIFF-truncated").unwrap();
        // A stray non-block file is ignored entirely.
        fs::write(dir.path().join("notes.txt"), b"x").unwrap();

        let found = scan_meeting_blocks(dir.path()).unwrap();
        assert_eq!(found.blocks.len(), 4);
        assert_eq!(
            found
                .blocks
                .iter()
                .map(|b| (b.track, b.index))
                .collect::<Vec<_>>(),
            vec![
                (Track::Mic, 1),
                (Track::Mic, 2),
                (Track::Mic, 3),
                (Track::System, 1)
            ]
        );
        assert_eq!(found.corrupt, vec![dir.path().join("mic-0004.wav")]);
    }

    #[test]
    fn scan_missing_directory_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        let found = scan_meeting_blocks(&dir.path().join("nope")).unwrap();
        assert!(found.blocks.is_empty());
        assert!(found.corrupt.is_empty());
    }

    #[test]
    fn audio_dirs_follow_the_data_model() {
        let root = Path::new("data");
        assert_eq!(
            meetings_root(root),
            Path::new("data").join("audio").join("meetings")
        );
        assert_eq!(
            meeting_audio_dir(root, "abc"),
            Path::new("data").join("audio").join("meetings").join("abc")
        );
    }
}
