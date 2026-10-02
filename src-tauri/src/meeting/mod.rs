//! Meeting detection support (F008), T-060 + T-061 scope.
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
//! - [`detector`]: the T-061 state machine — debounce, 10 min title memory,
//!   end-of-meeting, `detection_id` dedup (FR-008-02..06) — fed by the monitor
//!   and emitting [`detector::DetectorOutput`]s that [`run`] translates into
//!   `detector://meeting` IPC events (contracts.md §5).
//! - [`monitor`]: the tick loop honouring `meeting_detection_enabled`,
//!   `meeting_detection_paused_until_ms` and `offline_mode`
//!   ([`monitor::DetectionGate`]) before any classification runs.
//!
//! Also here (T-069): FR-008-13 auto-start — [`detector::wants_auto_start`]
//! decides and `emit_detector_outputs` fires `detector://start-requested`
//! alongside `detector://meeting`; FR-008-14 auto-stop consumes
//! `meeting_ended` ends at the session layer.
//!
//! Deliberately **out of scope** (other tasks): the toast window (T-062) and
//! the meeting session itself (T-064 — it listens for
//! `detector://start-requested`, emitted by
//! `commands::detector::detector_respond` and by the T-069 auto-start path).

pub(crate) mod app_icon;
mod classifier;
mod consent;
mod detector;
mod monitor;
mod window_snapshot;

// Re-exports consumed by the commands layer and the wiring below; the ones
// nothing names yet are kept public on purpose (platform sources).
#[allow(unused_imports)]
pub use classifier::{classify, current_exe_name, MeetingApp, RuleAction, BROWSER_EXES};
pub use consent::{MicUsage, MicUsageSource};
pub use detector::{
    Detection, DetectionSource, Detector, DetectorMeetingEvent, DetectorOutput,
    DetectorStartRequest, TickInput, DETECTOR_MEETING_EVENT, DETECTOR_START_REQUESTED_EVENT,
};
pub use monitor::DetectionGate;
#[allow(unused_imports)]
pub use monitor::{MicUsageMonitor, POLL_INTERVAL};
#[allow(unused_imports)]
pub use window_snapshot::{WindowInfo, WindowSnapshotSource};

#[cfg(target_os = "windows")]
pub use consent::ConsentStoreSource;
#[cfg(target_os = "windows")]
pub use window_snapshot::EnumWindowsSource;

use tauri::Manager as _;

/// Shared detector instance — managed as Tauri state so `detector_respond`
/// can address live detections by id while the monitor thread ticks it.
pub type SharedDetector = std::sync::Arc<std::sync::Mutex<Detector>>;

/// Create the detector, publish it as [`SharedDetector`] state and spawn the
/// microphone-usage monitor thread. The monitor is a no-op outside Windows
/// (macOS has its own S1/S2 equivalents, scheduled for v1.0 — see spec F008
/// notas técnicas); the managed state exists on every platform so the IPC
/// commands resolve uniformly.
pub fn start(app: &tauri::AppHandle) {
    let shared: SharedDetector = std::sync::Arc::new(std::sync::Mutex::new(Detector::new(
        current_exe_name().unwrap_or_default(),
    )));
    app.manage(shared.clone());
    #[cfg(target_os = "windows")]
    {
        let app = app.clone();
        match std::thread::Builder::new()
            .name("meeting-mic-monitor".to_string())
            .spawn(move || run(app, shared))
        {
            Ok(_) => log::info!("Meeting detector started (ConsentStore + window snapshots)"),
            Err(e) => log::error!("Failed to spawn meeting mic monitor: {e}"),
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = app;
        let _ = shared;
        log::debug!("Meeting mic monitor is not implemented on this platform yet");
    }
}

/// The monitor loop body. Runs until process exit (the thread is detached;
/// there is no shutdown signal wired in this task).
#[cfg(target_os = "windows")]
fn run(app: tauri::AppHandle, detector: SharedDetector) {
    use std::sync::atomic::AtomicBool;
    use std::sync::Arc;
    use std::time::Instant;

    let source = ConsentStoreSource::new();
    let windows = EnumWindowsSource;
    let self_exe = current_exe_name().unwrap_or_default();
    let stop = Arc::new(AtomicBool::new(false));

    // Rules change rarely; the connection is opened once and a failure
    // degrades to "no rules" instead of killing the monitor.
    let conn = open_rules_conn(&app);
    let mut was_suppressed = false;

    let gate = || {
        let settings = crate::settings::get_settings(&app);
        let suppressed = DetectionGate {
            enabled: settings.meeting_detection_enabled,
            paused_until_ms: settings.meeting_detection_paused_until_ms,
            offline_mode: settings.offline_mode,
        }
        .suppressed(crate::tray::now_unix_ms());
        if suppressed && !was_suppressed {
            // Entering suppression drops in-flight candidates/detections so
            // an un-pause starts clean — matching `MicUsageMonitor::tick`,
            // which clears its active set on suppressed ticks. The `Ended`
            // outputs are emitted so the toast layer closes anything open.
            let outputs = detector.lock().unwrap_or_else(|e| e.into_inner()).reset();
            emit_detector_outputs(&app, outputs);
        }
        was_suppressed = suppressed;
        suppressed
    };

    monitor::run_loop(
        source,
        gate,
        |usages: &[MicUsage], _changed: bool| {
            let rules = load_rules(conn.as_ref());
            let window_list = windows.snapshot();
            let classified = classify(usages, &window_list, &rules, &self_exe);
            let detect_any_call = crate::settings::get_settings(&app).detect_any_call_enabled;
            let outputs = detector
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .tick(&TickInput {
                    classified: &classified,
                    mic_usages: usages,
                    windows: &window_list,
                    rules: &rules,
                    detect_any_call,
                    now: Instant::now(),
                    now_unix_ms: crate::tray::now_unix_ms(),
                });
            emit_detector_outputs(&app, outputs);
        },
        stop,
    );
}

/// Translate detector outputs into `detector://meeting` IPC events
/// (contracts.md §5); emit failures are logged, never fatal.
///
/// FR-008-13 (T-069): a detection whose rule is `auto_start` — or any
/// detection while the global `meeting_auto_start` is on — additionally
/// fires `detector://start-requested` so the session starts without a
/// click (AC-008-04). Its `detector://meeting` action is reported as
/// `"auto_start"` so the toast renders the "Gravando · <App>"
/// confirmation instead of the ask prompt.
#[cfg(target_os = "windows")]
fn emit_detector_outputs(app: &tauri::AppHandle, outputs: Vec<DetectorOutput>) {
    use tauri::Emitter;

    let auto_start_global = crate::settings::get_settings(app).meeting_auto_start;
    for output in outputs {
        match output {
            DetectorOutput::Started(detection) => {
                let auto = detector::wants_auto_start(detection.action, auto_start_global);
                log::info!(
                    "Meeting detected: {} (id={}, exe={}, action={:?}, source={:?}, auto_start={auto})",
                    detection.app_label,
                    detection.detection_id,
                    detection.exe_name,
                    detection.action,
                    detection.source
                );
                let mut event = DetectorMeetingEvent::started(&detection);
                // Cached per exe path; known apps/browsers resolve to `None`
                // (the front-end embeds their logos).
                event.icon = app_icon::detection_icon(
                    &detection.app_label,
                    &detection.exe_name,
                    detection.exe_path.as_deref(),
                );
                if auto {
                    // The toast keys the "Gravando · <App>" confirmation
                    // face off `action == "auto_start"` — global-toggle
                    // starts must present identically to rule ones.
                    event.action = Some(RuleAction::AutoStart.as_str().to_string());
                }
                if let Err(e) = app.emit(DETECTOR_MEETING_EVENT, event) {
                    log::warn!("detector://meeting emit failed: {e}");
                }
                if auto {
                    if let Err(e) = app.emit(
                        DETECTOR_START_REQUESTED_EVENT,
                        DetectorStartRequest {
                            detection_id: detection.detection_id.clone(),
                            app_label: detection.app_label.clone(),
                            exe: detection.exe_name.clone(),
                            exe_path: detection.exe_path.clone(),
                            mic_only: false,
                            auto: true,
                        },
                    ) {
                        log::warn!("detector://start-requested emit failed: {e}");
                    }
                }
            }
            DetectorOutput::Ended {
                ref detection_id,
                meeting_over,
            } => {
                log::info!("Meeting ended (id={detection_id}, meeting_over={meeting_over})");
                let event = DetectorMeetingEvent::ended(detection_id, meeting_over);
                if let Err(e) = app.emit(DETECTOR_MEETING_EVENT, event) {
                    log::warn!("detector://meeting emit failed: {e}");
                }
            }
        }
    }
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

// Captura (T-063): [`blocks`] grava WAVs de 60 s por trilha sob
// `audio/meetings/<id>/` com fsync por bloco (FR-009-05); [`capture`] liga o
// mic compartilhado (`FrameTap::Raw` + `when_idle`) e o loopback WASAPI;
// [`recovery`] marca reuniões órfãs como `recovered` no startup.
// Transcrição ao vivo (T-065): [`live`] enfileira blocos selados num worker
// único com filas limitadas por trilha, segmenta por VAD em enunciados ≤30 s
// e persiste `meeting_segments` + `meeting://segment` (FR-009-15);
// `dictation` ali dentro é a coexistência com o ditado (FR-009-10).

pub mod blocks;
pub mod capture;
pub mod live;

// [postprocess] (T-067) escuta meeting://process-requested e roda
// transcrição de blocos pendentes → resumo → título sugerido (FR-009-16..22).
pub mod markdown;
pub mod postprocess;
pub mod recovery;
pub mod session;
pub mod view;
