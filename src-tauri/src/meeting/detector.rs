//! The meeting-detection state machine (spec F008, T-061): turns the
//! classifier's per-tick verdicts into durable *detections* with debounce,
//! browser title memory and end-of-meeting semantics, all as pure data so the
//! whole machine is testable on scripted [`Instant`] timelines.
//!
//! Rules implemented here (FR ids in-line):
//!
//! - **Debounce (FR-008-02)**: a classified match must stay *present* for
//!   [`DEBOUNCE`] before it becomes a detection (`DetectionStarted`).
//! - **Title memory (FR-008-02 + edge case "Meet em aba não ativa")**: a match
//!   that came through a window title (`MeetingApp::pid.is_some()`) keeps
//!   counting as present while the process still holds the mic and a title
//!   match was seen within [`TITLE_MEMORY`]. Memory only stretches the
//!   *presence* condition — it never postpones an end once the mic is gone.
//! - **End of meeting (FR-008-04)**: a live detection ends when (a) the
//!   process released the mic for [`MIC_RELEASE_GRACE`], (b) the process is
//!   gone — no mic usage *and* no window for that exe — or (c) for browsers
//!   only, the title is not matching *now* and the mic was released. A
//!   re-acquired mic inside the grace window cancels the pending end.
//! - **`detection_id` (FR-008-05)**: minted once at promotion as
//!   `exe:pid-or-path:since_ms`, so the same meeting can never emit a second
//!   `DetectionStarted` and a genuinely new meeting (new `since_ms`) fires
//!   again (AC-008-07).
//! - **"Detectar qualquer chamada" (FR-008-03)**: when `detect_any_call` is
//!   on, a usage held for [`ANY_CALL_HOLD`] by a non-browser, non-self
//!   process not covered by any rule match becomes a detection labelled with
//!   the exe file stem (product-name lookup is P1).
//! - **`Ignore` rules (AC-008-03)**: classifier matches carrying
//!   `RuleAction::Ignore` never become candidates and also suppress
//!   "any call" for the same exe.
//! - **`auto_start` rules (FR-008-13)**: emitted like any detection; the
//!   action rides on [`Detection::action`] so the consumer (T-069) can decide
//!   to start without asking. The machine itself never starts anything.
//! - **Self (FR-008-06)**: the classifier already drops our own exe before
//!   matches reach us; any-call re-checks `self_exe_name` on raw usages.
//!
//! Suppression boundary: the monitor gate (pause / offline / disabled) calls
//! [`Detector::reset`], which ends live detections but keeps the `dismissed`
//! set — a meeting the user dismissed does not re-toast after an un-pause.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use regex::Regex;
use serde::Serialize;

use crate::db::meetings::MeetingAppRule;

use super::classifier::{is_browser_exe, MeetingApp, RuleAction};
use super::consent::MicUsage;
use super::window_snapshot::WindowInfo;

/// FR-008-02: a match must be stable this long before it is a detection.
pub const DEBOUNCE: Duration = Duration::from_secs(5);
/// FR-008-02: how long a title-matched detection trusts the last title hit
/// while the mic stays held.
pub const TITLE_MEMORY: Duration = Duration::from_secs(10 * 60);
/// FR-008-04(a): a released mic only ends the meeting after this grace.
pub const MIC_RELEASE_GRACE: Duration = Duration::from_secs(15);
/// FR-008-03: mic-hold time required for a rule-less "any call" detection.
pub const ANY_CALL_HOLD: Duration = Duration::from_secs(10);

/// `detector://meeting` — contracts.md §5. Payloads:
/// `{detection_id, app_label, exe, icon, pid, action, started_at}` on start,
/// `{detection_id, ended: true}` on meeting end and
/// `{detection_id, ended: true, dismissed: true}` on user dismissal (the
/// `ended` flag rides along so the toast layer can treat dismissal as a plain
/// close; `dismissed` marks it as user-initiated).
pub const DETECTOR_MEETING_EVENT: &str = "detector://meeting";

/// `detector://start-requested` — emitted by `detector_respond` for
/// `start`/`start_mic_only`/`always`; the meeting session layer (T-064)
/// subscribes to it. Payload: [`DetectorStartRequest`].
pub const DETECTOR_START_REQUESTED_EVENT: &str = "detector://start-requested";

/// What produced a detection — a configured rule or the FR-008-03 fallback.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DetectionSource {
    /// A `meeting_app_rules` row matched (ask/auto_start).
    Rule,
    /// "Detectar qualquer chamada" caught an unlabelled mic holder.
    AnyCall,
}

/// A promoted detection — the unit `detector_respond` addresses by
/// `detection_id` (FR-008-05).
#[derive(Clone, Debug, PartialEq)]
pub struct Detection {
    /// Stable identity for this meeting: `exe:pid-or-path:since_ms`.
    pub detection_id: String,
    /// Rule label, or the exe file stem for any-call detections.
    pub app_label: String,
    /// Exe name as reported by the ConsentStore (`zoom.exe`, `MSTeams`).
    pub exe_name: String,
    /// Full exe path for unpackaged apps.
    pub exe_path: Option<String>,
    /// Pid of the window that satisfied the title rule, when known.
    pub pid: Option<u32>,
    /// The matched rule's action (`Ask` for any-call detections).
    pub action: RuleAction,
    /// Unix-ms when the mic was acquired (ConsentStore), or the promotion
    /// instant when the store did not report one.
    pub started_at_ms: i64,
    /// Rule vs. FR-008-03 fallback.
    pub source: DetectionSource,
}

/// Semantic outputs of a [`Detector::tick`]; `meeting::run` translates them
/// into `detector://meeting` emissions.
#[derive(Clone, Debug, PartialEq)]
pub enum DetectorOutput {
    /// A new meeting became a detection (debounce passed).
    Started(Detection),
    /// A live detection's meeting ended (FR-008-04) or was reset away.
    Ended { detection_id: String },
}

/// Everything the machine needs for one tick — all inputs are plain data so
/// tests can script time.
pub struct TickInput<'a> {
    /// This tick's [`classify`](super::classifier::classify) output — already
    /// self-filtered (FR-008-06) and including `Ignore` matches.
    pub classified: &'a [MeetingApp],
    /// Raw S1 snapshot (all mic-holding processes, including our own).
    pub mic_usages: &'a [MicUsage],
    /// S2 snapshot (visible windows).
    pub windows: &'a [WindowInfo],
    /// The raw `meeting_app_rules` rows — needed to re-check whether a
    /// browser's window title *still* matches after the mic was released
    /// (`classified` is empty by then, so FR-008-04(c) cannot read it there).
    pub rules: &'a [MeetingAppRule],
    /// `detect_any_call_enabled` setting (FR-008-03).
    pub detect_any_call: bool,
    /// Tick instant; only differences between instants are observed.
    pub now: Instant,
    /// Same instant as unix-ms — used only to stamp `started_at` fallbacks.
    pub now_unix_ms: i64,
}

/// IPC payload for `detector://meeting` (contracts.md §5). Optional fields
/// are omitted from the JSON so each variant serializes exactly to the
/// contract shapes; on the start variant `icon` stays `null` in v1 (icon
/// resolution is the toast lane's job, T-062).
#[derive(Clone, Debug, Serialize)]
pub struct DetectorMeetingEvent {
    pub detection_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub app_label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exe: Option<String>,
    /// App icon for the toast (FR-008-07) — `None`/omitted in v1; resolution
    /// is the toast lane's job (T-062).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
    /// `ask`/`auto_start`/`ignore` — the consumer (T-069) auto-starts on
    /// `auto_start`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub action: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub started_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ended: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dismissed: Option<bool>,
}

impl DetectorMeetingEvent {
    /// Start variant: `{detection_id, app_label, exe, pid, action, started_at}`
    /// (`icon` omitted in v1 — see the field docs).
    pub fn started(detection: &Detection) -> Self {
        Self {
            detection_id: detection.detection_id.clone(),
            app_label: Some(detection.app_label.clone()),
            exe: Some(detection.exe_name.clone()),
            icon: None,
            pid: detection.pid,
            action: Some(detection.action.as_str().to_string()),
            started_at: Some(detection.started_at_ms),
            ended: None,
            dismissed: None,
        }
    }

    /// End variant: `{detection_id, ended: true}`.
    pub fn ended(detection_id: &str) -> Self {
        Self {
            detection_id: detection_id.to_string(),
            app_label: None,
            exe: None,
            icon: None,
            pid: None,
            action: None,
            started_at: None,
            ended: Some(true),
            dismissed: None,
        }
    }

    /// Dismissal variant: `{detection_id, ended: true, dismissed: true}` —
    /// emitted by `detector_respond` for `dismiss`/`ignore_meeting`/`never`.
    /// `ended` rides along so the toast layer can treat dismissal as a plain
    /// close; `dismissed` marks it as user-initiated.
    pub fn dismissed(detection_id: &str) -> Self {
        Self {
            dismissed: Some(true),
            ..Self::ended(detection_id)
        }
    }
}

/// IPC payload for `detector://start-requested` (consumed by T-064).
#[derive(Clone, Debug, Serialize)]
pub struct DetectorStartRequest {
    pub detection_id: String,
    pub app_label: String,
    pub exe: String,
    /// `start_mic_only` → mic track only; `start`/`always` → full capture.
    pub mic_only: bool,
}

/// Identity of a tracked candidate: same app label behind the same process.
/// `exe`/`proc_key` are lowercased so ConsentStore case drift cannot split a
/// key.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct CandidateKey {
    label: String,
    exe: String,
    proc_key: String,
}

impl CandidateKey {
    fn of_app(app: &MeetingApp) -> Self {
        Self {
            label: app.label.clone(),
            exe: app.exe_name.to_lowercase(),
            proc_key: app
                .exe_path
                .as_deref()
                .unwrap_or(&app.exe_name)
                .to_lowercase(),
        }
    }

    fn of_usage(usage: &MicUsage, label: &str) -> Self {
        Self {
            label: label.to_string(),
            exe: usage.exe_name.to_lowercase(),
            proc_key: usage
                .exe_path
                .as_deref()
                .unwrap_or(&usage.key)
                .to_lowercase(),
        }
    }
}

/// A detection that has already been announced.
#[derive(Clone, Debug)]
struct LiveDetection {
    detection: Detection,
    /// `detector_respond` marked it dismissed — never emit `Ended` for it.
    dismissed: bool,
}

/// One candidate observed across ticks (pending or promoted).
struct Tracked {
    app_label: String,
    exe_name: String,
    exe_path: Option<String>,
    pid: Option<u32>,
    action: RuleAction,
    since_ms: Option<i64>,
    /// First tick the candidate was present (debounce/hold baseline).
    first_seen: Instant,
    /// Last tick whose match came through a window title (`pid.is_some()`);
    /// `Some` doubles as "this rule is title-based".
    last_title_match: Option<Instant>,
    /// First tick where the mic was observed released (grace baseline).
    released_since: Option<Instant>,
    live: Option<LiveDetection>,
    source: DetectionSource,
    /// `is_browser_exe(exe_name)` — enables the FR-008-04(c) early end.
    browser: bool,
}

impl Tracked {
    fn from_app(app: &MeetingApp, now: Instant) -> Self {
        Self {
            app_label: app.label.clone(),
            exe_name: app.exe_name.clone(),
            exe_path: app.exe_path.clone(),
            pid: app.pid,
            action: app.action,
            since_ms: app.since_ms,
            first_seen: now,
            last_title_match: app.pid.map(|_| now),
            released_since: None,
            live: None,
            source: DetectionSource::Rule,
            browser: is_browser_exe(&app.exe_name),
        }
    }

    fn any_call(usage: &MicUsage, label: String, now: Instant) -> Self {
        Self {
            app_label: label,
            exe_name: usage.exe_name.clone(),
            exe_path: usage.exe_path.clone(),
            pid: None,
            action: RuleAction::Ask,
            since_ms: usage.since_ms,
            first_seen: now,
            last_title_match: None,
            released_since: None,
            live: None,
            source: DetectionSource::AnyCall,
            browser: false, // browsers are never any-call candidates
        }
    }

    /// Refresh the cached match fields from this tick's `MeetingApp`.
    fn refresh(&mut self, app: &MeetingApp, now: Instant) {
        self.app_label.clone_from(&app.label);
        self.exe_path.clone_from(&app.exe_path);
        self.pid = app.pid;
        self.action = app.action;
        self.since_ms = app.since_ms;
        if app.pid.is_some() {
            self.last_title_match = Some(now);
        }
    }

    /// Whether `usage` is this candidate's process holding the mic. Exe name
    /// is always compared; exe path only when both sides carry one (packaged
    /// apps have no path, so name alone is the best available identity).
    fn holds_mic(&self, usage: &MicUsage) -> bool {
        if !self.exe_name.eq_ignore_ascii_case(&usage.exe_name) {
            return false;
        }
        match (&self.exe_path, &usage.exe_path) {
            (Some(a), Some(b)) => a.eq_ignore_ascii_case(b),
            _ => true,
        }
    }
}

/// The pure detector. Fed one [`TickInput`] per monitor tick; answers with
/// [`DetectorOutput`]s and keeps the bookkeeping needed by `detector_respond`.
pub struct Detector {
    /// Our own exe file name — re-checked on raw usages for any-call
    /// (classify already filters it out of `classified`; FR-008-06).
    self_exe_name: String,
    tracked: HashMap<CandidateKey, Tracked>,
    /// `detection_id`s the user dismissed/ignored this session; they never
    /// re-fire (AC-008-07) and survive `reset` so an un-pause stays quiet.
    dismissed: HashSet<String>,
    /// Fallback discriminator for `detection_id` when the ConsentStore did
    /// not report `since_ms`.
    next_id_seq: u64,
}

impl Detector {
    pub fn new(self_exe_name: String) -> Self {
        Self {
            self_exe_name,
            tracked: HashMap::new(),
            dismissed: HashSet::new(),
            next_id_seq: 0,
        }
    }

    /// Advance the machine one tick; returns the outputs to emit.
    pub fn tick(&mut self, input: &TickInput<'_>) -> Vec<DetectorOutput> {
        let mut outputs = Vec::new();

        // Index this tick's rule matches; `covered_exes` also collects
        // `Ignore` matches so they can suppress any-call for the same process.
        let mut current: HashMap<CandidateKey, &MeetingApp> = HashMap::new();
        let mut covered_exes: HashSet<String> = HashSet::new();
        for app in input.classified {
            covered_exes.insert(app.exe_name.to_lowercase());
            if app.action == RuleAction::Ignore {
                continue;
            }
            current.insert(CandidateKey::of_app(app), app);
        }

        // Upsert rule candidates.
        for (key, app) in &current {
            match self.tracked.get_mut(key) {
                Some(tracked) => {
                    tracked.refresh(app, input.now);
                }
                None => {
                    self.tracked
                        .insert(key.clone(), Tracked::from_app(app, input.now));
                }
            }
        }

        // FR-008-03: "detect any call" candidates from raw usages.
        if input.detect_any_call {
            for usage in input.mic_usages {
                if self.is_self(usage)
                    || is_browser_exe(&usage.exe_name)
                    || covered_exes.contains(&usage.exe_name.to_lowercase())
                {
                    continue;
                }
                let key = CandidateKey::of_usage(usage, &exe_stem(&usage.exe_name));
                self.tracked.entry(key).or_insert_with(|| {
                    Tracked::any_call(usage, exe_stem(&usage.exe_name), input.now)
                });
            }
        } else {
            // The toggle went off: drop pending any-call candidates and end
            // live ones so no stale toast lingers.
            let mut dead = Vec::new();
            for (key, tracked) in &self.tracked {
                if tracked.source == DetectionSource::AnyCall {
                    if let Some(live) = &tracked.live {
                        if !live.dismissed {
                            outputs.push(DetectorOutput::Ended {
                                detection_id: live.detection.detection_id.clone(),
                            });
                        }
                    }
                    dead.push(key.clone());
                }
            }
            for key in dead {
                self.tracked.remove(&key);
            }
        }

        // Per-candidate state transition.
        enum Transition {
            Keep,
            DropPending,
            Promote,
            End,
        }
        let keys: Vec<CandidateKey> = self.tracked.keys().cloned().collect();
        for key in keys {
            let transition = {
                let Some(tracked) = self.tracked.get_mut(&key) else {
                    continue;
                };
                let mic_held = input.mic_usages.iter().any(|u| tracked.holds_mic(u));
                let in_current = current.contains_key(&key);
                let present = match tracked.source {
                    DetectionSource::AnyCall => mic_held,
                    DetectionSource::Rule => {
                        in_current
                            || (mic_held
                                && tracked.last_title_match.is_some_and(|seen| {
                                    input.now.duration_since(seen) <= TITLE_MEMORY
                                }))
                    }
                };

                if tracked.live.is_some() {
                    if mic_held {
                        tracked.released_since = None;
                        Transition::Keep
                    } else {
                        // FR-008-04(b): the process is gone — no usage AND no
                        // window for that exe.
                        let exited = !input
                            .windows
                            .iter()
                            .any(|w| w.exe_name.eq_ignore_ascii_case(&tracked.exe_name));
                        // FR-008-04(c): for browsers the title is the
                        // presence signal — released mic + no window matching
                        // the rule's title ends immediately (memory is
                        // defined only while the mic is held, so it does not
                        // soften this). If a window still matches, the plain
                        // (a) grace applies instead.
                        let browser_done = tracked.browser && !title_still_matches(tracked, input);
                        if exited || browser_done {
                            Transition::End
                        } else {
                            // FR-008-04(a): the release grace — re-acquiring
                            // inside the window cancels the pending end.
                            let since = *tracked.released_since.get_or_insert(input.now);
                            if input.now.duration_since(since) >= MIC_RELEASE_GRACE {
                                Transition::End
                            } else {
                                Transition::Keep
                            }
                        }
                    }
                } else if !present {
                    Transition::DropPending
                } else {
                    let threshold = match tracked.source {
                        DetectionSource::Rule => DEBOUNCE,
                        DetectionSource::AnyCall => ANY_CALL_HOLD,
                    };
                    if input.now.duration_since(tracked.first_seen) >= threshold {
                        Transition::Promote
                    } else {
                        Transition::Keep
                    }
                }
            };

            match transition {
                Transition::Keep => {}
                Transition::DropPending => {
                    self.tracked.remove(&key);
                }
                Transition::End => {
                    if let Some(tracked) = self.tracked.remove(&key) {
                        if let Some(live) = tracked.live {
                            if !live.dismissed {
                                outputs.push(DetectorOutput::Ended {
                                    detection_id: live.detection.detection_id,
                                });
                            }
                        }
                    }
                }
                Transition::Promote => {
                    // Reserve the seq fallback before taking the &mut borrow
                    // (`mint_detection` itself is a free function).
                    if self.tracked.get(&key).is_some_and(|t| t.since_ms.is_none()) {
                        self.next_id_seq += 1;
                    }
                    let seq = self.next_id_seq;
                    if let Some(tracked) = self.tracked.get_mut(&key) {
                        let detection = mint_detection(tracked, seq, input.now_unix_ms);
                        let dismissed = self.dismissed.contains(&detection.detection_id);
                        let id = detection.detection_id.clone();
                        tracked.live = Some(LiveDetection {
                            detection: detection.clone(),
                            dismissed,
                        });
                        if dismissed {
                            log::debug!(
                                "Detection {id} re-emerged but is dismissed; staying quiet"
                            );
                        } else {
                            outputs.push(DetectorOutput::Started(detection));
                        }
                    }
                }
            }
        }

        outputs
    }

    /// The live detection with this id, if any — for `detector_respond`.
    pub fn find(&self, detection_id: &str) -> Option<&Detection> {
        self.tracked
            .values()
            .filter_map(|t| t.live.as_ref())
            .find(|live| live.detection.detection_id == detection_id)
            .map(|live| &live.detection)
    }

    /// Mark a detection dismissed (`dismiss`/`ignore_meeting`/`never`
    /// actions): its `Ended` output is suppressed and the id lands in the
    /// session dedup set so the same meeting never re-fires. Returns the
    /// detection (`None` → unknown id → `NotFound` at the command layer).
    pub fn dismiss(&mut self, detection_id: &str) -> Option<Detection> {
        let mut found = None;
        for tracked in self.tracked.values_mut() {
            let Some(live) = &mut tracked.live else {
                continue;
            };
            if live.detection.detection_id == detection_id {
                live.dismissed = true;
                found = Some(live.detection.clone());
            }
        }
        if found.is_some() {
            self.dismissed.insert(detection_id.to_string());
        }
        found
    }

    /// Drop all tracked candidates — called when the gate suppresses
    /// detection (paused, offline, `meeting_detection_enabled` off). Live,
    /// non-dismissed detections end cleanly (their `Ended` outputs are
    /// returned so the consumer can close toasts); the `dismissed` set
    /// survives on purpose.
    pub fn reset(&mut self) -> Vec<DetectorOutput> {
        let mut outputs = Vec::new();
        for (_, tracked) in self.tracked.drain() {
            if let Some(live) = tracked.live {
                if !live.dismissed {
                    outputs.push(DetectorOutput::Ended {
                        detection_id: live.detection.detection_id,
                    });
                }
            }
        }
        outputs
    }

    fn is_self(&self, usage: &MicUsage) -> bool {
        !self.self_exe_name.is_empty() && usage.exe_name.eq_ignore_ascii_case(&self.self_exe_name)
    }
}

/// Mint the promoted [`Detection`] for `tracked`; `started_at` prefers the
/// ConsentStore's acquisition timestamp and falls back to the tick's unix-ms
/// when the store reported none — the `seq` discriminator (pre-incremented
/// by the caller) keeps the `detection_id` unique in that case (FR-008-05).
fn mint_detection(tracked: &Tracked, seq: u64, now_unix_ms: i64) -> Detection {
    let proc = tracked
        .pid
        .map(|pid| pid.to_string())
        .or_else(|| tracked.exe_path.clone())
        .unwrap_or_else(|| tracked.exe_name.clone());
    let since = match tracked.since_ms {
        Some(ms) => ms.to_string(),
        None => format!("seq{seq}"),
    };
    Detection {
        detection_id: format!("{}:{}:{}", tracked.exe_name.to_lowercase(), proc, since),
        app_label: tracked.app_label.clone(),
        exe_name: tracked.exe_name.clone(),
        exe_path: tracked.exe_path.clone(),
        pid: tracked.pid,
        action: tracked.action,
        started_at_ms: tracked.since_ms.unwrap_or(now_unix_ms),
        source: tracked.source,
    }
}

/// Whether some visible window of `tracked`'s exe still matches its rule's
/// `title_pattern`. Only called for browser detections after the mic is gone
/// — `classified` is empty then by construction, so the rule rows are
/// consulted directly (same match semantics as
/// [`classifier`](super::classifier): an invalid pattern never matches).
fn title_still_matches(tracked: &Tracked, input: &TickInput<'_>) -> bool {
    input
        .rules
        .iter()
        .filter(|rule| {
            rule.exe.eq_ignore_ascii_case(&tracked.exe_name) && rule.label == tracked.app_label
        })
        .filter_map(|rule| rule.title_pattern.as_deref())
        .any(|pattern| {
            Regex::new(pattern).is_ok_and(|re| {
                input.windows.iter().any(|w| {
                    w.exe_name.eq_ignore_ascii_case(&tracked.exe_name) && re.is_match(&w.title)
                })
            })
        })
}

/// FR-008-03 label for a rule-less process: the exe file stem
/// (`audacity.exe` → `audacity`); product-name lookup is a P1 follow-up.
fn exe_stem(exe_name: &str) -> String {
    match exe_name.rsplit_once('.') {
        Some((stem, _ext)) if !stem.is_empty() => stem.to_string(),
        _ => exe_name.to_string(),
    }
}

#[cfg(test)]
mod tests;
