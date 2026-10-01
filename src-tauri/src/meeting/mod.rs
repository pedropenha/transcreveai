//! Meeting detection support (F008), T-060 scope.
//!
//! Pieces delivered here:
//!
//! - [`consent`]: **S1** — who holds the microphone, read from the Windows
//!   `CapabilityAccessManager\ConsentStore\microphone` registry tree behind
//!   the [`consent::MicUsageSource`] trait (`RegNotifyChangeKeyValue` for
//!   wake-ups, 2 s polling fallback — FR-008-01).
//! - [`window_snapshot`]: **S2** — visible top-level windows (pid, exe, title)
//!   via `EnumWindows`, behind [`window_snapshot::WindowSnapshotSource`].
//! - [`classifier`]: pure function mapping (mic usages × windows ×
//!   `meeting_app_rules` rows) → [`classifier::MeetingApp`] (FR-008-02 match
//!   semantics: exe match suffices, browsers additionally need a title hit).
//! - [`monitor`]: the tick loop honouring `meeting_detection_paused_until_ms`
//!   and `offline_mode` ([`monitor::DetectionGate`]) before any classification
//!   runs.
//!
//! Deliberately **out of scope** (later tasks): the debouncing / title-memory
//! state machine and IPC `detector://meeting` events (T-061), the toast window
//! (T-062), the meeting session itself (T-064). Today detections only hit the
//! log, which is enough to smoke-test the S1+S2 pipeline.

mod classifier;
mod consent;
mod monitor;
mod window_snapshot;

// Re-exports consumed by the T-061 detector (state machine + IPC events); the
// ones nothing names yet are kept public on purpose.
#[allow(unused_imports)]
pub use classifier::{classify, current_exe_name, MeetingApp, RuleAction};
pub use consent::{MicUsage, MicUsageSource};
pub use monitor::DetectionGate;
#[allow(unused_imports)]
pub use monitor::{MicUsageMonitor, POLL_INTERVAL};
#[allow(unused_imports)]
pub use window_snapshot::{WindowInfo, WindowSnapshotSource};

#[cfg(target_os = "windows")]
pub use consent::ConsentStoreSource;
#[cfg(target_os = "windows")]
pub use window_snapshot::EnumWindowsSource;

/// Spawn the microphone-usage monitor thread. No-op outside Windows (macOS has
/// its own S1/S2 equivalents, scheduled for v1.0 — see spec F008 notas
/// técnicas).
pub fn start(app: &tauri::AppHandle) {
    #[cfg(target_os = "windows")]
    {
        let app = app.clone();
        match std::thread::Builder::new()
            .name("meeting-mic-monitor".to_string())
            .spawn(move || run(app))
        {
            Ok(_) => log::info!("Meeting mic monitor started (ConsentStore + window snapshots)"),
            Err(e) => log::error!("Failed to spawn meeting mic monitor: {e}"),
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = app;
        log::debug!("Meeting mic monitor is not implemented on this platform yet");
    }
}

/// The monitor loop body. Runs until process exit (the thread is detached;
/// there is no shutdown signal wired in this task).
#[cfg(target_os = "windows")]
fn run(app: tauri::AppHandle) {
    use std::sync::atomic::AtomicBool;
    use std::sync::Arc;

    let source = ConsentStoreSource::new();
    let windows = EnumWindowsSource;
    let self_exe = current_exe_name().unwrap_or_default();
    let stop = Arc::new(AtomicBool::new(false));

    // Rules change rarely; the connection is opened once and a failure
    // degrades to "no rules" instead of killing the monitor.
    let conn = open_rules_conn(&app);
    let mut last_detected: Vec<MeetingApp> = Vec::new();

    let gate = || {
        let settings = crate::settings::get_settings(&app);
        DetectionGate {
            paused_until_ms: settings.meeting_detection_paused_until_ms,
            offline_mode: settings.offline_mode,
        }
        .suppressed(crate::tray::now_unix_ms())
    };

    monitor::run_loop(
        source,
        gate,
        |usages: &[MicUsage], changed: bool| {
            if usages.is_empty() {
                if !last_detected.is_empty() {
                    log::info!("Meeting detection: mic released by all tracked processes");
                    last_detected.clear();
                }
                return;
            }

            let rules = load_rules(conn.as_ref());
            let detected = classify(usages, &windows.snapshot(), &rules, &self_exe);
            if detected != last_detected {
                for app in &detected {
                    log::info!(
                        "Meeting app detected: {} (exe={}, pid={:?}, action={:?})",
                        app.label,
                        app.exe_name,
                        app.pid,
                        app.action
                    );
                }
                if detected.is_empty() && changed {
                    log::debug!(
                        "Mic in use by {:?} — no meeting_app_rules match",
                        usages
                            .iter()
                            .map(|u| u.exe_name.as_str())
                            .collect::<Vec<_>>()
                    );
                }
                last_detected = detected;
            }
        },
        stop,
    );
}

/// Fresh `meeting_app_rules` rows each tick so user edits apply live; `None`
/// (or an error) degrades to an empty rule set.
#[cfg(target_os = "windows")]
fn load_rules(conn: Option<&rusqlite::Connection>) -> Vec<crate::db::meetings::MeetingAppRule> {
    use crate::db::meetings::{MeetingAppRuleRepository, SqliteMeetingAppRuleRepository};
    conn.and_then(|c| {
        SqliteMeetingAppRuleRepository::new(c)
            .list()
            .map_err(|e| log::warn!("meeting_app_rules read failed: {e}"))
            .ok()
    })
    .unwrap_or_default()
}

#[cfg(target_os = "windows")]
fn open_rules_conn(app: &tauri::AppHandle) -> Option<rusqlite::Connection> {
    let dir = crate::portable::app_data_dir(app).ok()?;
    let path = crate::db::database_path(&dir).ok()?;
    crate::db::open_connection(&path)
        .map_err(|e| log::warn!("meeting rules db open failed: {e}"))
        .ok()
}

//! Captura (T-063): [`blocks`] grava WAVs de 60 s por trilha sob
//! `audio/meetings/<id>/` com fsync por bloco (FR-009-05); [`capture`] liga o
//! mic compartilhado (`FrameTap::Raw` + `when_idle`) e o loopback WASAPI;
//! [`recovery`] marca reuniões órfãs como `recovered` no startup.

pub mod blocks;
pub mod capture;
pub mod recovery;
