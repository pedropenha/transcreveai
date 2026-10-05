//! Foreground process image path (Windows).

use windows::Win32::Foundation::CloseHandle;
use windows::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};

/// Longest image path read (matches the 1024 buffer of the insertion probe).
const IMAGE_PATH_CAPACITY: usize = 1024;

/// Full image path of the process owning the foreground window, `None` for no
/// focus (lock screen, UAC) or when the process cannot be queried (higher
/// integrity). Only needs `PROCESS_QUERY_LIMITED_INFORMATION`.
pub(super) fn foreground_image_path() -> Option<String> {
    // SAFETY: no arguments; returns a possibly-null window handle.
    let hwnd = unsafe { GetForegroundWindow() };
    if hwnd.0.is_null() {
        return None;
    }
    let mut pid: u32 = 0;
    // SAFETY: `hwnd` is a window handle; `pid` is a valid out-pointer.
    unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
    if pid == 0 {
        return None;
    }
    // SAFETY: plain open of a process by id; the handle is closed below.
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }.ok()?;
    let mut buf = vec![0u16; IMAGE_PATH_CAPACITY];
    let mut len = buf.len() as u32;
    // SAFETY: `handle` is live; `buf` holds `len` UTF-16 units and the call
    // writes at most that many, updating `len` to the written length.
    let queried = unsafe {
        QueryFullProcessImageNameW(
            handle,
            PROCESS_NAME_WIN32,
            windows::core::PWSTR(buf.as_mut_ptr()),
            &mut len,
        )
    };
    // SAFETY: `handle` came from `OpenProcess` above and is closed once.
    let _ = unsafe { CloseHandle(handle) };
    queried.ok()?;
    Some(String::from_utf16_lossy(&buf[..len as usize]))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Whatever window has focus, a probe must not panic and any path it
    /// returns must be absolute.
    #[test]
    fn foreground_probe_returns_absolute_path_or_none() {
        if let Some(path) = foreground_image_path() {
            assert!(path.contains('\\'), "{path}");
        }
    }
}
