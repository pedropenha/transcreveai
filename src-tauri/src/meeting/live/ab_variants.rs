//! A/B fixture generator for meeting transcription quality (dev harness).
//!
//! Cuts a real meeting's WAV blocks the way the live pipeline does and writes
//! each cut as a WAV, so the same audio can be fed to several models through
//! `transcreve-ai --transcribe-file` and compared. `scripts/meeting-ab.ps1`
//! drives the transcription half and builds the report.
//!
//! The point is to separate two questions the practical test conflated: is the
//! *model* worse in meetings, or is the *slicing* worse? So the cuts come from
//! the production [`segment_utterances`] plus the production Silero VAD — not
//! a reimplementation — and only the variant axis changes:
//!
//! | variant  | what it is                                                    |
//! |----------|---------------------------------------------------------------|
//! | `exact`  | today's pipeline: spans cut rent to the VAD verdict            |
//! | `padded` | the cheap fix: the same spans + `pad_ms` both sides, as        |
//! |          | dictation's `SmoothedVad` pre-roll/hangover already does       |
//! | `whole`  | no slicing inside a block: the sealed 60 s block as one call   |
//! | `full`   | no slicing at all: every block of the track as one long call   |
//!
//! `exact` reproduces the *block pass* — the best case of today's pipeline.
//! The shipped output can be worse, because live chunks also hand the engine
//! sub-portions of an utterance that may start mid-word (`process.rs`).
//!
//! Run it (it is `#[ignore]`d — it needs a real meeting on disk):
//!
//! ```text
//! $env:MEETING_AB_DIR = "$env:APPDATA\br.com.creator4all.transcreve.ai\audio\meetings\<id>"
//! $env:MEETING_AB_OUT = "D:\ab"
//! cargo test -p transcreve-ai --lib ab_variants -- --ignored --nocapture
//! ```
//!
//! Env knobs: `MEETING_AB_TRACK` (`mic` | `system` | `both`, default `mic`),
//! `MEETING_AB_BLOCKS` (`3` or `1-5`, default all), `MEETING_AB_PAD_MS`
//! (default 450 — dictation's `VAD_PREFILL_MS`), `MEETING_AB_VARIANTS`
//! (comma list, default `exact,padded,whole`).

use std::fs;
use std::path::{Path, PathBuf};

use crate::audio_toolkit::constants::WHISPER_SAMPLE_RATE;
use crate::audio_toolkit::vad::VAD_PREFILL_MS;
use crate::audio_toolkit::{read_wav_samples, save_wav_file, SileroVad, VoiceActivityDetector};
use crate::managers::audio::SILERO_VAD_THRESHOLD;
use crate::meeting::blocks::{parse_block_filename, Track};

use super::segment::{segment_utterances, SegmenterConfig, Utterance};

/// One generated WAV, as the runner sees it in `manifest.json`.
struct Cut {
    variant: &'static str,
    track: &'static str,
    /// 0 for `full` (the cut spans every block).
    block: u32,
    /// 1-based within the block; 0 for `whole`/`full`.
    index: usize,
    start_ms: u64,
    end_ms: u64,
    file: String,
}

impl Cut {
    fn to_json(&self) -> String {
        format!(
            "{{\"variant\":\"{}\",\"track\":\"{}\",\"block\":{},\"index\":{},\
             \"start_ms\":{},\"end_ms\":{},\"file\":\"{}\"}}",
            self.variant,
            self.track,
            self.block,
            self.index,
            self.start_ms,
            self.end_ms,
            self.file.replace('\\', "\\\\")
        )
    }
}

fn env_var(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|v| !v.trim().is_empty())
}

/// `"3"` → `(3, 3)`; `"1-5"` → `(1, 5)`; absent → every block.
fn block_range() -> (u32, u32) {
    match env_var("MEETING_AB_BLOCKS") {
        None => (0, u32::MAX),
        Some(spec) => match spec.split_once('-') {
            Some((lo, hi)) => (
                lo.trim().parse().unwrap_or(0),
                hi.trim().parse().unwrap_or(u32::MAX),
            ),
            None => {
                let n = spec.trim().parse().unwrap_or(0);
                (n, n)
            }
        },
    }
}

fn tracks() -> Vec<Track> {
    match env_var("MEETING_AB_TRACK")
        .unwrap_or_else(|| "mic".into())
        .to_lowercase()
        .as_str()
    {
        "system" => vec![Track::System],
        "both" | "all" => Track::ALL.to_vec(),
        _ => vec![Track::Mic],
    }
}

/// Block files of one track, ordered by index and filtered by the range.
fn blocks_of(dir: &Path, track: Track) -> Vec<(u32, PathBuf)> {
    let (lo, hi) = block_range();
    let mut out: Vec<(u32, PathBuf)> = fs::read_dir(dir)
        .expect("MEETING_AB_DIR is not readable")
        .filter_map(|entry| {
            let path = entry.ok()?.path();
            let (t, index) = parse_block_filename(path.file_name()?.to_str()?)?;
            (t == track && index >= lo && index <= hi).then_some((index, path))
        })
        .collect();
    out.sort_by_key(|(index, _)| *index);
    out
}

/// Per-frame voice verdicts for `samples`, exactly as `Runner::segment` does:
/// fresh detector state, fail-open on an errored frame, zero-padded tail.
fn voiced_frames(vad: &mut dyn VoiceActivityDetector, samples: &[f32]) -> (Vec<bool>, usize) {
    vad.reset();
    let frame = vad.frame_samples();
    let mut voiced = Vec::with_capacity(samples.len().div_ceil(frame));
    for chunk in samples.chunks(frame) {
        if chunk.len() == frame {
            voiced.push(vad.is_voice(chunk).unwrap_or(true));
        } else {
            let mut padded = vec![0.0f32; frame];
            padded[..chunk.len()].copy_from_slice(chunk);
            voiced.push(vad.is_voice(&padded).unwrap_or(true));
        }
    }
    (voiced, frame)
}

/// Widen a span by `pad` samples on both sides, clamped to the buffer — the
/// `padded` variant, mirroring dictation's pre-roll + hangover.
fn widen(u: &Utterance, pad: usize, total: usize) -> (usize, usize) {
    (
        u.start_sample.saturating_sub(pad),
        (u.end_sample + pad).min(total),
    )
}

fn samples_to_ms(samples: usize) -> u64 {
    samples as u64 * 1_000 / WHISPER_SAMPLE_RATE as u64
}

#[allow(clippy::too_many_arguments)]
fn write_cut(
    out_root: &Path,
    variant: &'static str,
    track: Track,
    block: u32,
    index: usize,
    samples: &[f32],
    start_ms: u64,
    end_ms: u64,
) -> Cut {
    let dir = out_root.join(variant);
    fs::create_dir_all(&dir).expect("create variant dir");
    let name = if index == 0 {
        format!("{}-{block:04}.wav", track.label())
    } else {
        format!("{}-{block:04}-{index:03}.wav", track.label())
    };
    let path = dir.join(&name);
    save_wav_file(&path, samples).expect("write cut wav");
    Cut {
        variant,
        track: track.label(),
        block,
        index,
        start_ms,
        end_ms,
        file: path.to_string_lossy().to_string(),
    }
}

#[test]
#[ignore = "dev harness: needs MEETING_AB_DIR pointing at a real meeting"]
fn ab_variants() {
    let dir = PathBuf::from(
        env_var("MEETING_AB_DIR").expect("set MEETING_AB_DIR to a meetings/<id> directory"),
    );
    let out_root = PathBuf::from(env_var("MEETING_AB_OUT").expect("set MEETING_AB_OUT"));
    let pad_ms: u64 = env_var("MEETING_AB_PAD_MS")
        .and_then(|v| v.parse().ok())
        .unwrap_or(VAD_PREFILL_MS);
    let pad = pad_ms as usize * WHISPER_SAMPLE_RATE as usize / 1_000;
    let wanted = env_var("MEETING_AB_VARIANTS").unwrap_or_else(|| "exact,padded,whole".into());
    let wants = |v: &str| wanted.split(',').any(|w| w.trim().eq_ignore_ascii_case(v));

    let vad_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("resources")
        .join("models")
        .join("silero_vad_v4.onnx");
    let mut vad = SileroVad::new(vad_path, SILERO_VAD_THRESHOLD).expect("load silero vad");

    fs::create_dir_all(&out_root).expect("create MEETING_AB_OUT");
    let cfg = SegmenterConfig::default();
    let mut cuts: Vec<Cut> = Vec::new();

    for track in tracks() {
        let blocks = blocks_of(&dir, track);
        assert!(
            !blocks.is_empty(),
            "no {} blocks in {} for the requested range",
            track.label(),
            dir.display()
        );
        // `full`: every block of the track concatenated into one call — the
        // engine docs for these models state no practical length limit, so
        // this is the honest "stop slicing" measurement.
        let mut full: Vec<f32> = Vec::new();

        for (index, path) in &blocks {
            let samples = read_wav_samples(path).expect("read block wav");
            if wants("full") {
                full.extend_from_slice(&samples);
            }
            if wants("whole") {
                cuts.push(write_cut(
                    &out_root,
                    "whole",
                    track,
                    *index,
                    0,
                    &samples,
                    0,
                    samples_to_ms(samples.len()),
                ));
            }
            if !wants("exact") && !wants("padded") {
                continue;
            }

            let (voiced, frame) = voiced_frames(&mut vad, &samples);
            let utterances = segment_utterances(samples.len(), frame, &voiced, &cfg);
            println!(
                "{}-{:04}: {:.1}s → {} utterance(s)",
                track.label(),
                index,
                samples.len() as f64 / WHISPER_SAMPLE_RATE as f64,
                utterances.len()
            );
            for (i, u) in utterances.iter().enumerate() {
                if wants("exact") {
                    cuts.push(write_cut(
                        &out_root,
                        "exact",
                        track,
                        *index,
                        i + 1,
                        &samples[u.start_sample..u.end_sample],
                        u.start_ms,
                        u.end_ms,
                    ));
                }
                if wants("padded") {
                    let (start, end) = widen(u, pad, samples.len());
                    cuts.push(write_cut(
                        &out_root,
                        "padded",
                        track,
                        *index,
                        i + 1,
                        &samples[start..end],
                        samples_to_ms(start),
                        samples_to_ms(end),
                    ));
                }
            }
        }

        if wants("full") && !full.is_empty() {
            let end_ms = samples_to_ms(full.len());
            cuts.push(write_cut(&out_root, "full", track, 0, 0, &full, 0, end_ms));
        }
    }

    let manifest = format!(
        "{{\"meeting_dir\":\"{}\",\"pad_ms\":{},\"variants\":\"{}\",\"cuts\":[{}]}}",
        dir.to_string_lossy().replace('\\', "\\\\"),
        pad_ms,
        wanted,
        cuts.iter()
            .map(Cut::to_json)
            .collect::<Vec<_>>()
            .join(",\n")
    );
    let manifest_path = out_root.join("manifest.json");
    fs::write(&manifest_path, manifest).expect("write manifest");
    println!("\n{} cut(s) → {}", cuts.len(), manifest_path.display());
}
