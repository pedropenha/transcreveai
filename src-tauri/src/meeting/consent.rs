//! S1 signal for meeting detection (spec F008): which process is holding the
//! microphone right now, read from the Windows microphone ConsentStore:
//!
//! `HKCU\Software\Microsoft\Windows\CurrentVersion\CapabilityAccessManager\
//! ConsentStore\microphone`
//!
//! Child keys are packaged apps (named `<PackageFamilyName>!<AppId>`), plus a
//! `NonPackaged` key whose children are desktop exe paths with `#` standing in
//! for `\`. A leaf entry is actively using the microphone when
//! `LastUsedTimeStop == 0` (with a non-zero `LastUsedTimeStart`).
//!
//! The registry read is isolated behind [`MicUsageSource`] so the monitor and
//! the classifier are driven by scripted sources in tests (ECC TDD).

use std::io;
use std::time::Duration;

/// A process that is currently holding the microphone (FR-008-01, S1).
///
/// Emitted by [`MicUsageSource::snapshot`]; only *in-use* entries are reported
/// — release is observed by the entry disappearing from the next snapshot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MicUsage {
    /// Stable identity for diffing: the decoded exe path for desktop apps or
    /// the packaged subkey name for store apps.
    pub key: String,
    /// Full executable path (desktop apps only; `None` for packaged apps).
    pub exe_path: Option<String>,
    /// Name the app rules match against: the exe file name for desktop apps,
    /// or the tail after `!` of the packaged subkey (e.g. `MSTeams`).
    pub exe_name: String,
    /// Unix-ms timestamp converted from `LastUsedTimeStart` (FILETIME), when
    /// the process first acquired the mic.
    pub since_ms: Option<i64>,
}

/// Frontier trait behind which the real registry reader lives. Test monitors
/// feed scripted snapshots through a fake.
pub trait MicUsageSource: Send {
    /// Current set of processes holding the microphone.
    fn snapshot(&mut self) -> io::Result<Vec<MicUsage>>;

    /// Block until the store *may* have changed or `timeout` elapsed.
    ///
    /// The Windows implementation arms `RegNotifyChangeKeyValue` on the
    /// `microphone` key subtree (FR-008-01); the default just sleeps, which is
    /// the spec's 2 s polling fallback and what fakes use.
    fn wait_for_change(&mut self, timeout: Duration) -> io::Result<()> {
        std::thread::sleep(timeout);
        Ok(())
    }
}

/// Convert a Windows FILETIME (100 ns ticks since 1601-01-01) to unix epoch
/// milliseconds. `0` maps to `None` — the ConsentStore uses it as "unset".
pub fn filetime_to_unix_ms(filetime: u64) -> Option<i64> {
    if filetime == 0 {
        return None;
    }
    const FILETIME_UNIX_EPOCH_DELTA_MS: i64 = 11_644_473_600_000;
    let ms = (filetime / 10_000) as i64;
    Some(ms - FILETIME_UNIX_EPOCH_DELTA_MS)
}

/// Decode a `NonPackaged` subkey name: `C:#Program Files#Zoom#bin#Zoom.exe`
/// → `C:\Program Files\Zoom\bin\Zoom.exe`. The `#` encoding is unambiguous —
/// `\` never appears inside a path segment — so a straight replace is exact.
pub fn decode_nonpackaged_path(encoded: &str) -> String {
    encoded.replace('#', "\\")
}

/// Last path segment of a Windows path (`\` or `/` separated), preserving
/// case. Returns the whole string when there is no separator.
pub fn path_file_name(path: &str) -> &str {
    path.rsplit(['\\', '/']).next().unwrap_or(path)
}

/// Derive the `exe_name` for a packaged ConsentStore subkey: the tail after
/// the last `!` (`MSTeams_8wekyb3d8bbwe!MSTeams` → `MSTeams`), or the whole
/// name when there is no `!`.
fn packaged_app_name(subkey: &str) -> &str {
    subkey.rsplit('!').next().unwrap_or(subkey)
}

/// Build the [`MicUsage`] for one leaf entry. `subkey` is the raw child key
/// name; `packaged` distinguishes the `microphone\<subkey>` layout from
/// `microphone\NonPackaged\<subkey>`.
pub fn usage_from_entry(
    subkey: &str,
    packaged: bool,
    last_used_start: u64,
    last_used_stop: u64,
) -> Option<MicUsage> {
    // In use right now: a start timestamp recorded and no stop yet (S1).
    if last_used_stop != 0 || last_used_start == 0 {
        return None;
    }
    let since_ms = filetime_to_unix_ms(last_used_start);
    if packaged {
        Some(MicUsage {
            key: subkey.to_string(),
            exe_path: None,
            exe_name: packaged_app_name(subkey).to_string(),
            since_ms,
        })
    } else {
        let exe_path = decode_nonpackaged_path(subkey);
        Some(MicUsage {
            key: exe_path.clone(),
            exe_name: path_file_name(&exe_path).to_string(),
            exe_path: Some(exe_path),
            since_ms,
        })
    }
}

// ---------------------------------------------------------------------------
// Windows implementation (winreg for enumeration; RegNotifyChangeKeyValue for
// low-latency change notification, FR-008-01)
// ---------------------------------------------------------------------------

#[cfg(target_os = "windows")]
mod imp {
    use super::*;
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Registry::{
        RegCloseKey, RegNotifyChangeKeyValue, RegOpenKeyExW, HKEY, HKEY_CURRENT_USER, KEY_NOTIFY,
        REG_NOTIFY_CHANGE_LAST_SET, REG_NOTIFY_CHANGE_NAME, REG_NOTIFY_FILTER,
    };
    use windows::Win32::System::Threading::{CreateEventW, WaitForSingleObject, INFINITE};
    use winreg::enums::HKEY_CURRENT_USER as WINREG_HKCU;
    use winreg::RegKey;

    pub const MICROPHONE_CONSENT_PATH: &str =
        "Software\\Microsoft\\Windows\\CurrentVersion\\CapabilityAccessManager\\ConsentStore\\microphone";

    /// Reads the microphone ConsentStore. Each `snapshot` enumerates the leaf
    /// entries; `wait_for_change` blocks on `RegNotifyChangeKeyValue` (subtree
    /// watch) and falls back to a plain sleep when the notify handle could
    /// not be armed.
    pub struct ConsentStoreSource {
        notify_key: Option<NotifyKey>,
    }

    /// RAII wrapper for the `windows`-crate HKEY + event used by
    /// `RegNotifyChangeKeyValue`. Kept separate from the winreg handles so the
    /// two crates never share ownership of a handle.
    struct NotifyKey {
        key: HKEY,
        event: windows::Win32::Foundation::HANDLE,
    }

    impl Drop for NotifyKey {
        fn drop(&mut self) {
            unsafe {
                let _ = CloseHandle(self.event);
                let _ = RegCloseKey(self.key);
            }
        }
    }

    // HKEY/HANDLE are raw handles; the thread that owns the source uses them
    // exclusively. Required because `windows` 0.61 handle types are not Sync.
    unsafe impl Send for NotifyKey {}

    impl ConsentStoreSource {
        pub fn new() -> Self {
            Self {
                notify_key: open_notify_key()
                    .map_err(|e| {
                        log::warn!(
                            "RegNotifyChangeKeyValue unavailable on ConsentStore ({e}); \
                         falling back to {}s polling",
                            crate::meeting::monitor::POLL_INTERVAL.as_secs()
                        );
                        e
                    })
                    .ok(),
            }
        }
    }

    fn open_notify_key() -> io::Result<NotifyKey> {
        unsafe {
            let mut key = HKEY::default();
            let wide: Vec<u16> = MICROPHONE_CONSENT_PATH
                .encode_utf16()
                .chain(std::iter::once(0))
                .collect();
            let status = RegOpenKeyExW(
                HKEY_CURRENT_USER,
                PCWSTR(wide.as_ptr()),
                Some(0),
                KEY_NOTIFY,
                &mut key,
            );
            if status.0 != 0 {
                return Err(io::Error::from_raw_os_error(status.0 as i32));
            }
            let event = CreateEventW(None, true, false, PCWSTR::null()).map_err(|e| {
                let _ = RegCloseKey(key);
                io::Error::other(format!("CreateEventW failed: {e}"))
            })?;
            Ok(NotifyKey { key, event })
        }
    }

    impl MicUsageSource for ConsentStoreSource {
        fn snapshot(&mut self) -> io::Result<Vec<MicUsage>> {
            let root = RegKey::predef(WINREG_HKCU).open_subkey(MICROPHONE_CONSENT_PATH)?;
            let mut usages = Vec::new();
            for sub in root.enum_keys().flatten() {
                if sub.eq_ignore_ascii_case("NonPackaged") {
                    continue;
                }
                let Ok(child) = root.open_subkey(&sub) else {
                    continue;
                };
                let Some(entry) = leaf_usage(&child, &sub, true) else {
                    continue;
                };
                usages.push(entry);
            }
            if let Ok(nonpackaged) = root.open_subkey("NonPackaged") {
                for sub in nonpackaged.enum_keys().flatten() {
                    let Ok(child) = nonpackaged.open_subkey(&sub) else {
                        continue;
                    };
                    if let Some(entry) = leaf_usage(&child, &sub, false) {
                        usages.push(entry);
                    }
                }
            }
            Ok(usages)
        }

        fn wait_for_change(&mut self, timeout: Duration) -> io::Result<()> {
            match &self.notify_key {
                Some(notify) => unsafe {
                    // Watch subkey create/delete AND value writes (the
                    // LastUsedTimeStop flip is a value write).
                    let status = RegNotifyChangeKeyValue(
                        notify.key,
                        true,
                        REG_NOTIFY_FILTER(REG_NOTIFY_CHANGE_NAME.0 | REG_NOTIFY_CHANGE_LAST_SET.0),
                        Some(notify.event),
                        true,
                    );
                    if status.0 != 0 {
                        // Notify arm failed — degrade to the polling cadence
                        // so a transient error doesn't spin the loop.
                        std::thread::sleep(timeout);
                        return Ok(());
                    }
                    let ms = timeout
                        .as_millis()
                        .min(INFINITE as u128 - 1)
                        .try_into()
                        .unwrap_or(INFINITE - 1);
                    let _ = WaitForSingleObject(notify.event, ms); // timeout is a normal outcome
                    Ok(())
                },
                None => {
                    std::thread::sleep(timeout);
                    Ok(())
                }
            }
        }
    }

    fn leaf_usage(key: &RegKey, subkey: &str, packaged: bool) -> Option<MicUsage> {
        let start: u64 = key.get_value("LastUsedTimeStart").ok()?;
        let stop: u64 = key.get_value("LastUsedTimeStop").unwrap_or(0);
        if matches!(
            key.get_value::<String, _>("Value"),
            Ok(v) if v.eq_ignore_ascii_case("deny")
        ) {
            return None;
        }
        usage_from_entry(subkey, packaged, start, stop)
    }

    impl Default for ConsentStoreSource {
        fn default() -> Self {
            Self::new()
        }
    }
}

#[cfg(target_os = "windows")]
pub use imp::ConsentStoreSource;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filetime_zero_is_unset() {
        assert_eq!(filetime_to_unix_ms(0), None);
    }

    #[test]
    fn filetime_converts_to_unix_ms() {
        // 2024-01-01T00:00:00Z = unix 1704067200000 ms.
        let filetime = (1_704_067_200_000i64 + 11_644_473_600_000) as u64 * 10_000;
        assert_eq!(filetime_to_unix_ms(filetime), Some(1_704_067_200_000));
    }

    #[test]
    fn decodes_nonpackaged_path() {
        assert_eq!(
            decode_nonpackaged_path("C:#Program Files#Zoom#bin#Zoom.exe"),
            "C:\\Program Files\\Zoom\\bin\\Zoom.exe"
        );
    }

    #[test]
    fn extracts_file_name_from_windows_path() {
        assert_eq!(path_file_name("C:\\x\\y\\Teams.exe"), "Teams.exe");
        assert_eq!(path_file_name("Zoom.exe"), "Zoom.exe");
        assert_eq!(path_file_name("C:/a/b/msedge.exe"), "msedge.exe");
    }

    #[test]
    fn in_use_requires_start_and_no_stop() {
        let usage = usage_from_entry("C:#x#Zoom.exe", false, 10_000_000, 0)
            .expect("stop == 0 with a start means in use");
        assert_eq!(usage.exe_name, "Zoom.exe");
        assert_eq!(usage.exe_path.as_deref(), Some("C:\\x\\Zoom.exe"));

        // Stopped: a real stop timestamp clears the in-use flag.
        assert!(usage_from_entry("C:#x#Zoom.exe", false, 10_000_000, 20_000_000).is_none());
        // Never started.
        assert!(usage_from_entry("C:#x#Zoom.exe", false, 0, 0).is_none());
    }

    #[test]
    fn packaged_entry_uses_tail_after_bang() {
        let usage = usage_from_entry("MSTeams_8wekyb3d8bbwe!MSTeams", true, 10_000_000, 0)
            .expect("packaged usage");
        assert_eq!(usage.exe_name, "MSTeams");
        assert_eq!(usage.exe_path, None);
        assert_eq!(usage.key, "MSTeams_8wekyb3d8bbwe!MSTeams");
    }
}
