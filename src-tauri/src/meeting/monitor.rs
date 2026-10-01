//! [`MicUsageMonitor`] — the FR-008-01 loop that turns [`MicUsageSource`]
//! snapshots into *transitions* (mic acquired / released by some process) and
//! polls [`crate::meeting::window_snapshot::WindowSnapshotSource`] + the
//! classifier while the mic is held.
//!
//! Change notification: `MicUsageSource::wait_for_change` arms
//! `RegNotifyChangeKeyValue` on Windows; on every wake — event or timeout — the
//! snapshot is re-read, so the loop satisfies both the notify path and the
//! 2 s polling fallback with one code path.
//!
//! Gates (T-007 settings): while detection is paused
//! (`meeting_detection_paused_until_ms` in the future) or `offline_mode` is
//! on, snapshots are ignored entirely — nothing is classified and any tracked
//! state is dropped so an un-pause starts clean.

use std::collections::HashMap;
use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use crate::tray;

use super::consent::{MicUsage, MicUsageSource};

/// Fallback polling cadence (FR-008-01). Also the maximum latency for noticing
/// that detection got un-paused or that a `wait_for_change` arm failed.
pub const POLL_INTERVAL: Duration = Duration::from_secs(2);

/// Whether detection runs right now. Built fresh each tick from settings.
#[derive(Clone, Copy, Debug)]
pub struct DetectionGate {
    /// `meeting_detection_enabled` (FR-008-15 "Detectar reuniões") — the
    /// feature's master switch (T-061).
    pub enabled: bool,
    /// `meeting_detection_paused_until_ms` (T-007 tray pause, FR-010-14).
    pub paused_until_ms: Option<i64>,
    /// `privacy.offline_mode` (FR-011-08): while on, the detector stays quiet
    /// — detection itself is local, but the pipeline it gates (toast →
    /// notetaker → STT) is not.
    pub offline_mode: bool,
}

impl DetectionGate {
    /// Current instant in unix-ms is supplied so the gate stays pure/testable.
    pub fn suppressed(&self, now_ms: i64) -> bool {
        !self.enabled
            || tray::meeting_detection_is_paused(self.paused_until_ms, now_ms)
            || self.offline_mode
    }
}

/// Tracks which processes were seen holding the mic and reports transitions.
/// Pure over snapshots — the platform source is a trait parameter.
pub struct MicUsageMonitor<S> {
    source: S,
    /// key → usage of every process seen holding the mic last tick.
    active: HashMap<String, MicUsage>,
}

impl<S: MicUsageSource> MicUsageMonitor<S> {
    pub fn new(source: S) -> Self {
        Self {
            source,
            active: HashMap::new(),
        }
    }

    /// One iteration: returns `Some(in_use)` when the set of mic-holding
    /// processes changed since the previous un-suppressed tick, `None`
    /// otherwise. `suppressed` drops tracked state and reports no changes.
    pub fn tick(&mut self, suppressed: bool) -> io::Result<Option<Vec<MicUsage>>> {
        if suppressed {
            self.active.clear();
            return Ok(None);
        }

        let snapshot = self.source.snapshot()?;
        let next: HashMap<String, MicUsage> =
            snapshot.into_iter().map(|u| (u.key.clone(), u)).collect();

        if next == self.active {
            return Ok(None);
        }
        self.active = next;
        Ok(Some(self.active_usages()))
    }

    /// Current in-use set — what the last un-suppressed tick saw.
    pub fn active_usages(&self) -> Vec<MicUsage> {
        self.active.values().cloned().collect()
    }

    /// Access to the wrapped source (for `wait_for_change`).
    pub fn source_mut(&mut self) -> &mut S {
        &mut self.source
    }
}

/// Blocking run loop: every un-suppressed tick calls
/// `on_tick(current_in_use, changed)` — `changed` marks transitions so the
/// consumer can log/emit only on edges while still re-classifying on every
/// tick (window titles move independently of the mic set). Suppressed ticks
/// are silent. Runs until `stop` flips; meant for a dedicated thread.
pub fn run_loop<S, Gate, Tick>(source: S, mut gate: Gate, mut on_tick: Tick, stop: Arc<AtomicBool>)
where
    S: MicUsageSource,
    Gate: FnMut() -> bool,
    Tick: FnMut(&[MicUsage], bool),
{
    let mut monitor = MicUsageMonitor::new(source);
    while !stop.load(Ordering::Relaxed) {
        let suppressed = gate();
        match monitor.tick(suppressed) {
            Ok(changed) if !suppressed => on_tick(&monitor.active_usages(), changed.is_some()),
            Ok(_) => {}
            Err(e) => log::warn!("Mic usage snapshot failed: {e}"),
        }
        if let Err(e) = monitor.source_mut().wait_for_change(POLL_INTERVAL) {
            log::debug!("ConsentStore wait failed ({e}); sleeping instead");
            std::thread::sleep(POLL_INTERVAL);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::sync::atomic::AtomicUsize;
    use std::sync::Mutex;

    fn usage(key: &str, exe_name: &str) -> MicUsage {
        MicUsage {
            key: key.to_string(),
            exe_path: Some(format!("C:\\apps\\{exe_name}")),
            exe_name: exe_name.to_string(),
            since_ms: Some(1_000),
        }
    }

    /// Scripted source: each `snapshot` pops the next scripted set; when the
    /// script runs dry it replays the last one (steady state).
    struct ScriptSource {
        script: Mutex<VecDeque<Vec<MicUsage>>>,
        last: Vec<MicUsage>,
        waits: Arc<AtomicUsize>,
    }

    impl ScriptSource {
        fn new(script: Vec<Vec<MicUsage>>, waits: Arc<AtomicUsize>) -> Self {
            Self {
                script: Mutex::new(script.into()),
                last: Vec::new(),
                waits,
            }
        }
    }

    impl MicUsageSource for ScriptSource {
        fn snapshot(&mut self) -> io::Result<Vec<MicUsage>> {
            if let Some(next) = self.script.lock().unwrap().pop_front() {
                self.last = next;
            }
            Ok(self.last.clone())
        }

        fn wait_for_change(&mut self, _timeout: Duration) -> io::Result<()> {
            self.waits.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    #[test]
    fn gate_suppressed_when_disabled_paused_or_offline() {
        let now = 1_000_000i64;
        let gate = |enabled, until, offline| DetectionGate {
            enabled,
            paused_until_ms: until,
            offline_mode: offline,
        };
        assert!(!gate(true, None, false).suppressed(now));
        assert!(!gate(true, Some(now), false).suppressed(now));
        assert!(!gate(true, Some(now - 1), false).suppressed(now));
        assert!(gate(true, Some(now + 1), false).suppressed(now));
        assert!(gate(true, None, true).suppressed(now));
        assert!(gate(true, Some(now + 1), true).suppressed(now));
        // The FR-008-15 master switch gates everything.
        assert!(gate(false, None, false).suppressed(now));
        assert!(gate(false, Some(now - 1), false).suppressed(now));
    }

    #[test]
    fn tick_reports_only_transitions() {
        let mut monitor = MicUsageMonitor::new(ScriptSource::new(
            vec![
                vec![],                       // idle
                vec![usage("z", "Zoom.exe")], // zoom grabs mic
                vec![usage("z", "Zoom.exe")], // steady — no event
                vec![],                       // released
                vec![],                       // still idle — no event
            ],
            Arc::new(AtomicUsize::new(0)),
        ));

        assert_eq!(monitor.tick(false).unwrap(), None);
        let emit = monitor.tick(false).unwrap().expect("zoom started");
        assert_eq!(emit.len(), 1);
        assert_eq!(emit[0].exe_name, "Zoom.exe");
        assert_eq!(monitor.tick(false).unwrap(), None);
        assert_eq!(
            monitor.tick(false).unwrap(),
            Some(vec![]),
            "release emits the now-empty set"
        );
        assert_eq!(monitor.tick(false).unwrap(), None);
    }

    #[test]
    fn suppression_drops_state_and_stays_quiet() {
        let mut monitor = MicUsageMonitor::new(ScriptSource::new(
            vec![
                vec![usage("z", "Zoom.exe")], // seen while active
                vec![usage("z", "Zoom.exe")], // suppressed — must not emit
                vec![usage("z", "Zoom.exe")], // un-suppressed: set is "new" again
            ],
            Arc::new(AtomicUsize::new(0)),
        ));

        assert_eq!(monitor.tick(false).unwrap().unwrap().len(), 1);
        assert_eq!(monitor.tick(true).unwrap(), None);
        // After un-pausing, the ongoing usage is reported again so the
        // detector can pick it up.
        assert_eq!(monitor.tick(false).unwrap().unwrap().len(), 1);
    }

    #[test]
    fn suppression_while_idle_stays_idle() {
        // Suppressed ticks do not consume the source at all — the script
        // frames below are read by the two un-suppressed ticks only.
        let mut monitor = MicUsageMonitor::new(ScriptSource::new(
            vec![
                vec![],                       // idle
                vec![usage("z", "Zoom.exe")], // zoom grabs the mic mid-unpause
            ],
            Arc::new(AtomicUsize::new(0)),
        ));

        assert_eq!(monitor.tick(true).unwrap(), None);
        assert_eq!(monitor.tick(true).unwrap(), None);
        assert_eq!(monitor.tick(false).unwrap(), None, "still idle");
        assert_eq!(monitor.tick(false).unwrap().unwrap().len(), 1);
        assert!(!monitor.active_usages().is_empty());
    }

    #[test]
    fn run_loop_reports_every_tick_and_stops() {
        let waits = Arc::new(AtomicUsize::new(0));
        let source = ScriptSource::new(vec![vec![], vec![usage("z", "Zoom.exe")]], waits.clone());
        let stop = Arc::new(AtomicBool::new(false));
        // (in_use_len, changed) per un-suppressed tick.
        let ticks = Arc::new(Mutex::new(Vec::<(usize, bool)>::new()));
        let ticks2 = ticks.clone();
        let stop2 = stop.clone();

        run_loop(
            source,
            || false,
            move |usages: &[MicUsage], changed: bool| {
                let mut ticks = ticks2.lock().unwrap();
                ticks.push((usages.len(), changed));
                if ticks.len() >= 4 {
                    stop2.store(true, Ordering::SeqCst);
                }
            },
            stop,
        );

        let ticks = ticks.lock().unwrap();
        // idle (no change) → zoom grabs mic (change) → steady (no change) ×2.
        assert_eq!(
            ticks.as_slice(),
            &[(0, false), (1, true), (1, false), (1, false)]
        );
    }
}
