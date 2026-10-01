//! S2 signal for meeting detection (spec F008): a snapshot of the visible
//! top-level windows — pid, exe name and title — used to correlate a mic-using
//! browser process with a meeting tab (`Meet - …`, `meet.google.com`).
//!
//! Enumeration (`EnumWindows` + `GetWindowTextW` + `GetWindowThreadProcessId`
//! and `QueryFullProcessImageNameW`) lives behind
//! [`WindowSnapshotSource`] — the classifier is exercised with synthetic
//! snapshots in tests.

/// One visible top-level window at snapshot time.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WindowInfo {
    /// Owning process id.
    pub pid: u32,
    /// Executable file name (e.g. `chrome.exe`), matched case-insensitively
    /// against `meeting_app_rules.exe`.
    pub exe_name: String,
    /// Window title (`GetWindowTextW`).
    pub title: String,
}

/// Frontier trait: real enumeration is Windows-only; tests feed synthetic
/// vectors.
pub trait WindowSnapshotSource: Send {
    /// All currently visible top-level windows with a non-empty title.
    fn snapshot(&self) -> Vec<WindowInfo>;
}

#[cfg(target_os = "windows")]
mod imp {
    use super::{WindowInfo, WindowSnapshotSource};
    use crate::meeting::consent::path_file_name;
    use windows::core::{BOOL, PWSTR};
    use windows::Win32::Foundation::{CloseHandle, HWND, LPARAM};
    use windows::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32,
        PROCESS_QUERY_LIMITED_INFORMATION,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetWindowTextW, GetWindowThreadProcessId, IsWindowVisible,
    };

    /// Enumerates visible top-level windows on this session's desktop.
    pub struct EnumWindowsSource;

    impl Default for EnumWindowsSource {
        fn default() -> Self {
            Self
        }
    }

    impl WindowSnapshotSource for EnumWindowsSource {
        fn snapshot(&self) -> Vec<WindowInfo> {
            let mut hwnds: Vec<HWND> = Vec::new();
            unsafe {
                // EnumWindows yields top-level windows only; visibility and a
                // non-empty title filter the noise (tray icons, shell windows).
                let _ = EnumWindows(
                    Some(enum_wnd_proc),
                    LPARAM(&mut hwnds as *mut Vec<HWND> as isize),
                );
            }
            hwnds.iter().filter_map(|&hwnd| read_window(hwnd)).collect()
        }
    }

    unsafe extern "system" fn enum_wnd_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let list = unsafe { &mut *(lparam.0 as *mut Vec<HWND>) };
        if unsafe { IsWindowVisible(hwnd) }.as_bool() {
            list.push(hwnd);
        }
        BOOL(1) // continue enumeration
    }

    fn read_window(hwnd: HWND) -> Option<WindowInfo> {
        let mut buf = vec![0u16; 512];
        let len = unsafe { GetWindowTextW(hwnd, &mut buf) };
        if len <= 0 {
            return None;
        }
        let title = String::from_utf16_lossy(&buf[..len as usize]);
        if title.trim().is_empty() {
            return None;
        }

        let mut pid: u32 = 0;
        unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
        if pid == 0 {
            return None;
        }

        let exe_name = process_image_name(pid)
            .map(|path| path_file_name(&path).to_string())
            .unwrap_or_default();

        Some(WindowInfo {
            pid,
            exe_name,
            title,
        })
    }

    /// Full image path of `pid` (`QueryFullProcessImageNameW`), or `None` when
    /// the process cannot be opened (e.g. elevated/system processes — those
    /// windows keep an empty `exe_name` and simply never match a rule).
    fn process_image_name(pid: u32) -> Option<String> {
        unsafe {
            let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
            let _guard = HandleGuard(handle);
            let mut buf = vec![0u16; 1024];
            let mut len = buf.len() as u32;
            QueryFullProcessImageNameW(
                handle,
                PROCESS_NAME_WIN32,
                PWSTR(buf.as_mut_ptr()),
                &mut len,
            )
            .ok()?;
            Some(String::from_utf16_lossy(&buf[..len as usize]))
        }
    }

    struct HandleGuard(windows::Win32::Foundation::HANDLE);

    impl Drop for HandleGuard {
        fn drop(&mut self) {
            unsafe {
                let _ = CloseHandle(self.0);
            }
        }
    }
}

#[cfg(target_os = "windows")]
pub use imp::EnumWindowsSource;
