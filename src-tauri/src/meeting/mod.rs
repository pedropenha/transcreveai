//! Meeting notetaker domain (F009). T-063 delivers the capture plumbing the
//! session (T-064) and live transcription (T-065) build on:
//!
//! - [`blocks`] — incremental 60 s WAV blocks per track under
//!   `audio/meetings/<id>/`, fsync'd per block so a crash loses at most the
//!   in-flight block (FR-009-05, NFR-009-03).
//! - [`capture`] — [`capture::MeetingCapture`] wires the shared mic stream
//!   (`FrameTap::Raw` + `when_idle`, so meeting frames keep flowing between
//!   dictations) and the WASAPI system-audio loopback into per-track writers.
//! - [`recovery`] — startup detection of meetings orphaned by a dead process
//!   (`recording`/`paused` → `recovered`) and scanning of their block files.

pub mod blocks;
pub mod capture;
pub mod recovery;
