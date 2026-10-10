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
        // lparam is the `&mut Vec<HWND>` `snapshot` passed to EnumWindows; the
        // callback runs on the calling thread for the duration of that call.
        let list = &mut *(lparam.0 as *mut Vec<HWND>);
        if IsWindowVisible(hwnd).as_bool() {
            list.push(hwnd);
        }
        BOOL(1) // continue enumeration
    }

    fn read_window(hwnd: HWND) -> Option<WindowInfo> {
        let mut buf = vec![0u16; 512];
        // `buf` is a valid 512-u16 out-buffer and `pid` a valid out-param.
        let (len, pid) = unsafe {
            let len = GetWindowTextW(hwnd, &mut buf);
            let mut pid: u32 = 0;
            GetWindowThreadProcessId(hwnd, Some(&mut pid));
            (len, pid)
        };
        if len <= 0 {
            return None;
        }
        let title = String::from_utf16_lossy(&buf[..len as usize]);
        if title.trim().is_empty() {
            return None;
        }
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

// ---------------------------------------------------------------------------
// macOS implementation (CGWindowListCopyWindowInfo)
// ---------------------------------------------------------------------------

#[cfg(target_os = "macos")]
mod imp {
    //! macOS S2 (spec F008 notas técnicas): `CGWindowListCopyWindowInfo`
    //! returns the on-screen windows with `kCGWindowOwnerPID` /
    //! `kCGWindowOwnerName` / `kCGWindowName` (the title).
    //!
    //! `kCGWindowName` for other apps' windows requires Screen Recording
    //! (same permission the meeting loopback needs): without it titles are
    //! absent, so exe-only rules (Zoom, Teams) still fire but browser rules
    //! (Meet) can't see a title to match — the source logs that hint once.
    //!
    //! `exe_name` resolution mirrors the mic source: pid →
    //! `NSRunningApplication.bundleURL` → outermost `.app` file name, so a
    //! window of `Google Chrome Helper (Renderer)` still reports
    //! `Google Chrome.app`. `kCGWindowOwnerName` is the fallback.

    use std::collections::HashMap;
    use std::sync::atomic::{AtomicBool, Ordering};

    use objc2::rc::{autoreleasepool, Retained};
    use objc2::runtime::AnyObject;
    use objc2_app_kit::NSRunningApplication;
    use objc2_core_foundation::CFRetained;
    use objc2_core_graphics::{
        kCGNullWindowID, CGPreflightScreenCaptureAccess, CGWindowListCopyWindowInfo,
        CGWindowListOption,
    };
    use objc2_foundation::{NSArray, NSDictionary, NSNumber, NSString};

    use super::super::consent::{outermost_app, path_file_name};
    use super::{WindowInfo, WindowSnapshotSource};

    /// `CGWindowListCopyWindowInfo` reader. `titles_hint_logged` (interior
    /// mutability — `snapshot` takes `&self`) warns once when titles are
    /// being withheld for lack of Screen Recording.
    pub struct MacWindowSource {
        titles_hint_logged: AtomicBool,
    }

    impl MacWindowSource {
        pub fn new() -> Self {
            Self {
                titles_hint_logged: AtomicBool::new(false),
            }
        }
    }

    impl Default for MacWindowSource {
        fn default() -> Self {
            Self::new()
        }
    }

    type WindowDict = NSDictionary<NSString, AnyObject>;

    /// `dict[key]` as an `i32`, when the value is an `NSNumber`.
    fn number(dict: &WindowDict, key: &NSString) -> Option<i32> {
        dict.objectForKey(key)?
            .downcast::<NSNumber>()
            .ok()
            .map(|n| n.as_i32())
    }

    /// `dict[key]` as a `String`, when the value is an `NSString`.
    fn string(dict: &WindowDict, key: &NSString) -> Option<String> {
        dict.objectForKey(key)?
            .downcast::<NSString>()
            .ok()
            .map(|s| s.to_string())
    }

    /// Outermost `.app` file name for `pid` (`Google Chrome.app`), `None`
    /// when the owner has no resolvable bundle path.
    fn exe_name_for_pid(pid: i32) -> Option<String> {
        let app = NSRunningApplication::runningApplicationWithProcessIdentifier(pid)?;
        let path = app.bundleURL()?.path()?.to_string();
        let path = outermost_app(&path);
        Some(path_file_name(path).to_string()).filter(|name| !name.is_empty())
    }

    impl WindowSnapshotSource for MacWindowSource {
        fn snapshot(&self) -> Vec<WindowInfo> {
            autoreleasepool(|_| self.inner())
        }
    }

    impl MacWindowSource {
        fn inner(&self) -> Vec<WindowInfo> {
            // Layer-0 windows only: the list otherwise carries menu-bar items,
            // status icons and other chrome that can never host a meeting tab.
            let options =
                CGWindowListOption::OptionOnScreenOnly | CGWindowListOption::ExcludeDesktopElements;
            let Some(list) = CGWindowListCopyWindowInfo(options, kCGNullWindowID) else {
                return Vec::new();
            };
            // SAFETY: elements of the returned array are CFDictionaryRef —
            // toll-free bridged with NSDictionary. `into_raw` hands the +1
            // reference to the Retained below, so ownership is preserved.
            let windows: Retained<NSArray<WindowDict>> =
                unsafe { Retained::from_raw(CFRetained::into_raw(list).as_ptr().cast()) }
                    .unwrap_or_default();

            let key_pid = NSString::from_str("kCGWindowOwnerPID");
            let key_name = NSString::from_str("kCGWindowName");
            let key_owner = NSString::from_str("kCGWindowOwnerName");
            let key_layer = NSString::from_str("kCGWindowLayer");

            let has_screen_recording = CGPreflightScreenCaptureAccess();
            let mut missing_titles = false;
            // pid → exe_name, resolved once per snapshot (a browser owns
            // dozens of windows).
            let mut exe_by_pid: HashMap<i32, Option<String>> = HashMap::new();
            let mut out = Vec::new();
            for dict in windows.iter() {
                if number(&dict, &key_layer).unwrap_or(0) != 0 {
                    continue;
                }
                let Some(pid) = number(&dict, &key_pid) else {
                    continue;
                };
                let Some(title) = string(&dict, &key_name) else {
                    missing_titles = true;
                    continue;
                };
                if title.trim().is_empty() {
                    continue;
                }
                let exe_name = exe_by_pid
                    .entry(pid)
                    .or_insert_with(|| exe_name_for_pid(pid).or_else(|| string(&dict, &key_owner)))
                    .clone()
                    .unwrap_or_default();
                out.push(WindowInfo {
                    pid: pid as u32,
                    exe_name,
                    title,
                });
            }
            if missing_titles
                && !has_screen_recording
                && !self.titles_hint_logged.swap(true, Ordering::Relaxed)
            {
                log::warn!(
                    "Meeting detection: window titles need Screen Recording — \
                     browser rules (Google Meet) stay quiet until it is granted"
                );
            }
            out
        }
    }
}

#[cfg(target_os = "macos")]
pub use imp::MacWindowSource;
