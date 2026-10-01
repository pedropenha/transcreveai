//! Free-disk-space checks for downloads and imports (FR-003-06).
//!
//! [`check_disk_space`] is the pure decision — a friendly, quota-aware error —
//! and [`available_space`] is the platform probe behind it. A probe that can't
//! answer returns `None` and the caller proceeds: a failed stat must never
//! block a download that would have fit.

use anyhow::Result;
use log::warn;
use std::path::{Path, PathBuf};

/// Human-readable byte count for error messages ("512 MB" / "1.5 GB").
fn format_bytes(bytes: u64) -> String {
    const MB: u64 = 1024 * 1024;
    const GB: u64 = 1024 * MB;
    if bytes >= GB {
        format!("{:.1} GB", bytes as f64 / GB as f64)
    } else {
        format!("{} MB", bytes.div_ceil(MB))
    }
}

/// The disk-space gate (FR-003-06): fail fast with an actionable error naming
/// both sides of the shortfall instead of filling the disk mid-download.
pub(super) fn check_disk_space(needed_bytes: u64, available_bytes: u64, what: &str) -> Result<()> {
    if needed_bytes > available_bytes {
        return Err(anyhow::anyhow!(
            "Not enough free disk space for {what}: needs about {} but only {} are available. Free up space and try again.",
            format_bytes(needed_bytes),
            format_bytes(available_bytes),
        ));
    }
    Ok(())
}

/// [`check_disk_space`] against the real filesystem, or `Ok(())` when the
/// probe can't answer — a stat failure must not block a download.
pub(super) fn ensure_disk_space(dir: &Path, needed_bytes: u64, what: &str) -> Result<()> {
    if needed_bytes == 0 {
        return Ok(());
    }
    match available_space(dir) {
        Some(free) => check_disk_space(needed_bytes, free, what),
        None => {
            warn!(
                "Could not determine free disk space under {:?}; skipping pre-download check",
                dir
            );
            Ok(())
        }
    }
}

/// Free bytes available to an unprivileged process on the filesystem hosting
/// `dir`, or `None` when probing fails. `dir` need not exist — the probe walks
/// up to the nearest existing ancestor first (the models dir / HF cache dir is
/// routinely absent before the first download).
pub(super) fn available_space(dir: &Path) -> Option<u64> {
    imp::available_space(&nearest_existing_dir(dir))
}

/// First ancestor (or `path` itself) that exists as a directory; `path` when
/// none do, in which case the platform probe will simply fail to `None`.
fn nearest_existing_dir(path: &Path) -> PathBuf {
    let mut candidate = path;
    loop {
        if candidate.is_dir() {
            return candidate.to_path_buf();
        }
        match candidate.parent() {
            Some(parent) => candidate = parent,
            None => return path.to_path_buf(),
        }
    }
}

#[cfg(target_os = "windows")]
mod imp {
    use std::path::Path;
    use windows::core::HSTRING;
    use windows::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;

    pub fn available_space(dir: &Path) -> Option<u64> {
        let path = HSTRING::from(dir.as_os_str());
        let mut free_for_caller: u64 = 0;
        // SAFETY: `free_for_caller` is a valid out-param; the path is a
        // null-terminated UTF-16 string owned by `path`.
        unsafe {
            GetDiskFreeSpaceExW(
                windows::core::PCWSTR::from_raw(path.as_ptr()),
                Some(&mut free_for_caller),
                None,
                None,
            )
            .ok()?;
        }
        Some(free_for_caller)
    }
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
mod imp {
    use std::os::unix::ffi::OsStrExt;
    use std::path::Path;

    /// `statvfs` — `f_bavail * f_frsize` is the space a non-root caller may
    /// actually use (reserved blocks excluded).
    pub fn available_space(dir: &Path) -> Option<u64> {
        let c_path = std::ffi::CString::new(dir.as_os_str().as_bytes()).ok()?;
        let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
        // SAFETY: `stat` is a valid statvfs out-param; `c_path` is NUL-terminated.
        let rc = unsafe { libc::statvfs(c_path.as_ptr(), &mut stat) };
        if rc != 0 {
            return None;
        }
        Some(u64::from(stat.f_bavail) * stat.f_frsize as u64)
    }
}

#[cfg(not(any(target_os = "windows", target_os = "linux", target_os = "macos")))]
mod imp {
    use std::path::Path;

    pub fn available_space(_dir: &Path) -> Option<u64> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn check_passes_when_space_fits() {
        assert!(check_disk_space(100, 100, "model").is_ok());
        assert!(check_disk_space(0, 0, "model").is_ok());
    }

    #[test]
    fn check_fails_with_friendly_amounts() {
        let err = check_disk_space(5 * 1024 * 1024 * 1024, 512 * 1024 * 1024, "Whisper Turbo")
            .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("Not enough free disk space"), "{msg}");
        assert!(msg.contains("5.0 GB"), "{msg}");
        assert!(msg.contains("512 MB"), "{msg}");
        assert!(msg.contains("Whisper Turbo"), "{msg}");
    }

    #[test]
    fn probe_answers_for_existing_dir() {
        let tmp = tempfile::TempDir::new().unwrap();
        let free = available_space(tmp.path());
        assert!(free.is_some());
        assert!(free.unwrap() > 0);
    }

    #[test]
    fn probe_walks_to_existing_ancestor() {
        let tmp = tempfile::TempDir::new().unwrap();
        let missing = tmp.path().join("not/yet/created");
        assert_eq!(nearest_existing_dir(&missing), tmp.path().to_path_buf());
    }
}
