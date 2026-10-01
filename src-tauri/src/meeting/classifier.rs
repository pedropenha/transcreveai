//! Pure classifier (spec F008, FR-008-02): given the processes currently
//! holding the microphone (S1), a snapshot of visible windows (S2) and the
//! `meeting_app_rules` rows (T-004), decide which known meeting app — if any —
//! is running a call.
//!
//! No state, no I/O: the detector state machine (T-061 — debounce, 10 min
//! title memory, end-of-meeting) feeds it fresh snapshots every tick and owns
//! persistence of results. That keeps this function fully testable with
//! synthetic timelines.

use regex::Regex;

use crate::db::meetings::MeetingAppRule;

use super::consent::{path_file_name, MicUsage};
use super::window_snapshot::WindowInfo;

/// What the user wants the detector to do when this app is matched
/// (`meeting_app_rules.action`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuleAction {
    /// Show the "Reunião detectada" toast (default posture).
    Ask,
    /// Start the notetaker without asking.
    AutoStart,
    /// Never prompt for this app.
    Ignore,
}

impl RuleAction {
    /// Parse the `action` column; unknown values fall back to `Ask` so a
    /// malformed row still surfaces a prompt instead of silently doing
    /// nothing.
    pub fn parse(action: &str) -> Self {
        match action {
            "auto_start" => Self::AutoStart,
            "ignore" => Self::Ignore,
            _ => Self::Ask,
        }
    }
}

/// A meeting app identified behind the mic right now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MeetingApp {
    /// Rule label shown to the user ("Zoom", "Google Meet").
    pub label: String,
    /// Exe name that matched the rule (`zoom.exe`, `chrome.exe`).
    pub exe_name: String,
    /// Full exe path when the ConsentStore is a `NonPackaged` entry.
    pub exe_path: Option<String>,
    /// Pid of the window that satisfied the title pattern, when the match went
    /// through S2 (`None` for exe-only matches — the ConsentStore does not
    /// carry pids).
    pub pid: Option<u32>,
    /// Matched rule's action; `Ignore` matches are reported so the detector
    /// can suppress "detect any call" (FR-008-03) for the same process.
    pub action: RuleAction,
    /// Unix-ms instant the mic was acquired, per the ConsentStore.
    pub since_ms: Option<i64>,
}

/// Browser exes never match on exe alone (FR-008-02): they use the mic for
/// plenty of non-meeting purposes, so a rule for a browser exe only fires when
/// a window of that exe also matches the rule's `title_pattern`.
///
/// The set doubles as the Meet seeds' exe list; user-added rules for other
/// browsers (e.g. `opera.exe`) still need a `title_pattern` to fire — a
/// documented requirement rather than a hidden heuristic.
pub const BROWSER_EXES: &[&str] = &[
    "chrome.exe",
    "msedge.exe",
    "firefox.exe",
    "brave.exe",
    "arc.exe",
    "opera.exe",
    "operagx.exe",
    "vivaldi.exe",
    "chromium.exe",
    "iexplore.exe",
];

fn is_browser_exe(exe_name: &str) -> bool {
    BROWSER_EXES
        .iter()
        .any(|browser| exe_name.eq_ignore_ascii_case(browser))
}

fn is_self(usage: &MicUsage, self_exe_name: &str) -> bool {
    !self_exe_name.is_empty() && usage.exe_name.eq_ignore_ascii_case(self_exe_name)
}

/// Whether `rule` explains `usage` given the current windows.
///
/// - Exe match is case-insensitive on the file name.
/// - `title_pattern == None`: matches on exe alone, *unless* the exe is a
///   browser (those always need a title hit).
/// - `title_pattern == Some(re)`: some window of the same exe must match `re`.
///   An invalid regex never matches (logged, not fatal — rules are user data).
fn rule_matches(
    rule: &MeetingAppRule,
    pattern: Option<&Regex>,
    usage: &MicUsage,
    windows: &[WindowInfo],
) -> Option<Option<u32>> {
    if !usage.exe_name.eq_ignore_ascii_case(&rule.exe) {
        return None;
    }
    match pattern {
        None if is_browser_exe(&usage.exe_name) => None,
        None => Some(None),
        Some(re) => windows
            .iter()
            .filter(|w| w.exe_name.eq_ignore_ascii_case(&rule.exe))
            .find(|w| re.is_match(&w.title))
            .map(|w| Some(w.pid)),
    }
}

/// Compile a rule's `title_pattern` once per classify call; `None` patterns
/// stay `None` and invalid regexes compile to `Some(None)` → never match.
fn compile_pattern(rule: &MeetingAppRule) -> Option<Option<Regex>> {
    rule.title_pattern.as_deref().map(|pattern| {
        Regex::new(pattern)
            .map_err(|e| {
                log::warn!(
                    "Ignoring invalid title_pattern {:?} of meeting_app_rules {}: {}",
                    pattern,
                    rule.id,
                    e
                );
                e
            })
            .ok()
    })
}

/// The meeting apps identifiable behind the microphone right now, in rule
/// order. The caller's own exe (FR-008-01 "ignorar o próprio executável") is
/// filtered out before matching; FR-008-06 (self-recording) follows from that
/// since our recording holds the mic under our own exe name.
pub fn classify(
    mic_usages: &[MicUsage],
    windows: &[WindowInfo],
    rules: &[MeetingAppRule],
    self_exe_name: &str,
) -> Vec<MeetingApp> {
    let patterns: Vec<Option<Option<Regex>>> = rules.iter().map(compile_pattern).collect();

    let mut detected = Vec::new();
    for usage in mic_usages.iter().filter(|u| !is_self(u, self_exe_name)) {
        for (rule, pattern) in rules.iter().zip(&patterns) {
            let Some(pid) = rule_matches(
                rule,
                pattern.as_ref().and_then(|p| p.as_ref()),
                usage,
                windows,
            ) else {
                continue;
            };
            detected.push(MeetingApp {
                label: rule.label.clone(),
                exe_name: usage.exe_name.clone(),
                exe_path: usage.exe_path.clone(),
                pid,
                action: RuleAction::parse(&rule.action),
                since_ms: usage.since_ms,
            });
        }
    }
    detected
}

/// Own exe file name for `self_exe_name` — `None` when the current exe cannot
/// be resolved (detection still runs; FR-008-01's self-ignore just has no
/// name to compare).
pub fn current_exe_name() -> Option<String> {
    std::env::current_exe()
        .ok()
        .map(|p| path_file_name(&p.to_string_lossy()).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn usage(exe_name: &str) -> MicUsage {
        MicUsage {
            key: exe_name.to_string(),
            exe_path: Some(format!("C:\\apps\\{exe_name}")),
            exe_name: exe_name.to_string(),
            since_ms: Some(1_000),
        }
    }

    fn window(exe_name: &str, title: &str) -> WindowInfo {
        WindowInfo {
            pid: 42,
            exe_name: exe_name.to_string(),
            title: title.to_string(),
        }
    }

    fn rule(exe: &str, label: &str, pattern: Option<&str>, action: &str) -> MeetingAppRule {
        MeetingAppRule {
            id: format!("test-{label}-{exe}"),
            exe: exe.to_string(),
            title_pattern: pattern.map(str::to_string),
            label: label.to_string(),
            action: action.to_string(),
            builtin: true,
        }
    }

    /// The shipped v1 seeds (migration 10), duplicated here so classifier
    /// tests read like the spec's rule table.
    fn builtin_rules() -> Vec<MeetingAppRule> {
        vec![
            rule("Zoom.exe", "Zoom", None, "ask"),
            rule("ms-teams.exe", "Microsoft Teams", None, "ask"),
            rule("Teams.exe", "Microsoft Teams", None, "ask"),
            rule("MSTeams", "Microsoft Teams", None, "ask"),
            rule(
                "chrome.exe",
                "Google Meet",
                Some(r"^Meet -|meet\.google\.com"),
                "ask",
            ),
            rule(
                "msedge.exe",
                "Google Meet",
                Some(r"^Meet -|meet\.google\.com"),
                "ask",
            ),
            rule("CiscoCollabHost.exe", "Webex", None, "ask"),
            rule("webexmta.exe", "Webex", None, "ask"),
        ]
    }

    #[test]
    fn zoom_matches_on_exe_alone() {
        let found = classify(
            &[usage("Zoom.exe")],
            &[],
            &builtin_rules(),
            "transcreve-ai.exe",
        );
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].label, "Zoom");
        assert_eq!(found[0].action, RuleAction::Ask);
        assert_eq!(found[0].pid, None);
    }

    #[test]
    fn exe_match_is_case_insensitive() {
        let found = classify(&[usage("ZOOM.EXE")], &[], &builtin_rules(), "app.exe");
        assert_eq!(found[0].exe_name, "ZOOM.EXE");
        assert_eq!(found[0].label, "Zoom");
    }

    #[test]
    fn browser_needs_a_matching_window_title() {
        // Chrome on the mic for something else (web dictation, recording) does
        // not trigger Meet (FR-008-02).
        let usage = usage("chrome.exe");
        let youtube = window("chrome.exe", "YouTube - Google Chrome");
        assert!(classify(
            std::slice::from_ref(&usage),
            &[youtube],
            &builtin_rules(),
            "app.exe"
        )
        .is_empty());

        let meet = window("chrome.exe", "Meet - abc-defg-hij");
        let found = classify(&[usage], &[meet], &builtin_rules(), "app.exe");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].label, "Google Meet");
        assert_eq!(found[0].pid, Some(42));
    }

    #[test]
    fn browser_without_title_pattern_never_matches() {
        // A browser rule with no pattern can never fire — the exe alone is not
        // enough signal.
        let rules = vec![rule("opera.exe", "Custom", None, "ask")];
        let usage = usage("opera.exe");
        let windows = vec![window("opera.exe", "Weekly sync")];
        assert!(classify(&[usage], &windows, &rules, "app.exe").is_empty());
    }

    #[test]
    fn window_of_a_different_exe_does_not_satisfy_the_rule() {
        // Firefox shows a Meet tab but Chrome is the one on the mic — no
        // detection (the mic-using process must own the matching window).
        let rules = builtin_rules();
        let found = classify(
            &[usage("chrome.exe")],
            &[window("firefox.exe", "Meet - abc-defg-hij")],
            &rules,
            "app.exe",
        );
        assert!(found.is_empty());
    }

    #[test]
    fn own_exe_is_ignored_even_with_a_rule() {
        let rules = vec![rule("transcreve-ai.exe", "Self", None, "ask")];
        let found = classify(
            &[usage("transcreve-ai.exe")],
            &[],
            &rules,
            "transcreve-ai.exe",
        );
        assert!(found.is_empty());
    }

    #[test]
    fn ignore_rules_report_their_action() {
        let rules = vec![rule("Discord.exe", "Discord", None, "ignore")];
        let found = classify(&[usage("discord.exe")], &[], &rules, "app.exe");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].action, RuleAction::Ignore);
    }

    #[test]
    fn auto_start_action_is_preserved() {
        let rules = vec![rule("Zoom.exe", "Zoom", None, "auto_start")];
        let found = classify(&[usage("Zoom.exe")], &[], &rules, "app.exe");
        assert_eq!(found[0].action, RuleAction::AutoStart);
    }

    #[test]
    fn invalid_title_pattern_is_skipped_not_fatal() {
        let rules = vec![rule("chrome.exe", "Broken", Some("([unclosed"), "ask")];
        let found = classify(
            &[usage("chrome.exe")],
            &[window("chrome.exe", "anything")],
            &rules,
            "app.exe",
        );
        assert!(found.is_empty());
    }

    #[test]
    fn nonbrowser_rule_with_pattern_requires_the_title() {
        let rules = vec![rule("Zoom.exe", "Zoom", Some(r"Zoom Meeting"), "ask")];
        let usage = usage("Zoom.exe");
        assert!(classify(std::slice::from_ref(&usage), &[], &rules, "app.exe").is_empty());
        let found = classify(
            &[usage],
            &[window("Zoom.exe", "Zoom Meeting")],
            &rules,
            "app.exe",
        );
        assert_eq!(found.len(), 1);
    }

    #[test]
    fn packaged_teams_key_matches_msteams_rule() {
        // New Teams is packaged: the ConsentStore key is
        // `MSTeams_…!MSTeams`, so exe_name is the `!` tail.
        let mut usage = usage("MSTeams");
        usage.exe_path = None;
        usage.key = "MSTeams_8wekyb3d8bbwe!MSTeams".to_string();
        let found = classify(&[usage], &[], &builtin_rules(), "app.exe");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].label, "Microsoft Teams");
    }

    #[test]
    fn unrelated_mic_usage_yields_nothing() {
        let found = classify(&[usage("audacity.exe")], &[], &builtin_rules(), "app.exe");
        assert!(found.is_empty());
    }

    #[test]
    fn rule_action_parse_defaults_to_ask() {
        assert_eq!(RuleAction::parse("ask"), RuleAction::Ask);
        assert_eq!(RuleAction::parse("auto_start"), RuleAction::AutoStart);
        assert_eq!(RuleAction::parse("ignore"), RuleAction::Ignore);
        assert_eq!(RuleAction::parse("bogus"), RuleAction::Ask);
    }
}
