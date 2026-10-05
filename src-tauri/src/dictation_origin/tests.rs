use super::*;

#[test]
fn friendly_names_for_common_apps() {
    for (exe, expected) in [
        ("Claude.exe", "Claude"),
        ("chrome.exe", "Chrome"),
        ("msedge.exe", "Edge"),
        ("firefox.exe", "Firefox"),
        ("Code.exe", "Visual Studio Code"),
        ("slack.exe", "Slack"),
        ("Discord.exe", "Discord"),
        ("ms-teams.exe", "Teams"),
        ("Teams.exe", "Teams"),
        ("Zoom.exe", "Zoom"),
        ("notepad.exe", "Bloco de Notas"),
        ("notepad++.exe", "Notepad++"),
        ("WindowsTerminal.exe", "Terminal"),
        ("OUTLOOK.EXE", "Outlook"),
        ("WINWORD.EXE", "Word"),
        ("EXCEL.EXE", "Excel"),
    ] {
        assert_eq!(friendly_app_name(exe), expected, "{exe}");
    }
}

#[test]
fn friendly_name_falls_back_to_capitalized_stem() {
    assert_eq!(friendly_app_name("audacity.exe"), "Audacity");
    assert_eq!(friendly_app_name("obs64.EXE"), "Obs64");
    assert_eq!(friendly_app_name("noext"), "Noext");
    assert_eq!(friendly_app_name("épico.exe"), "Épico");
}

#[test]
fn friendly_name_accepts_full_paths_and_blank() {
    assert_eq!(friendly_app_name(r"C:\Apps\Claude.exe"), "Claude");
    assert_eq!(friendly_app_name("C:/Apps/slack.exe"), "Slack");
    assert_eq!(friendly_app_name(""), "");
    assert_eq!(friendly_app_name(".exe"), "");
}

#[test]
fn origin_from_safe_path_keeps_name_label_and_path() {
    let origin = DictationOrigin::from_image_path(r" C:\Program Files\Claude\Claude.exe ").unwrap();
    assert_eq!(origin.exe_name, "Claude.exe");
    assert_eq!(origin.app_name, "Claude");
    assert_eq!(
        origin.exe_path.as_deref(),
        Some(r"C:\Program Files\Claude\Claude.exe")
    );
}

#[test]
fn origin_from_unsafe_path_drops_only_the_path() {
    for bad in [r"\\server\share\Tool.exe", r"C:\a\..\Tool.exe", "Tool.exe"] {
        let origin = DictationOrigin::from_image_path(bad).unwrap();
        assert_eq!(origin.exe_name, "Tool.exe", "{bad}");
        assert_eq!(origin.app_name, "Tool", "{bad}");
        assert_eq!(origin.exe_path, None, "{bad}");
    }
}

#[test]
fn origin_needs_a_file_name() {
    assert_eq!(DictationOrigin::from_image_path(""), None);
    assert_eq!(DictationOrigin::from_image_path(r"C:\apps\"), None);
}

fn origin(name: &str) -> DictationOrigin {
    DictationOrigin::from_image_path(&format!(r"C:\apps\{name}")).unwrap()
}

#[test]
fn registry_hands_each_session_its_own_origin_once() {
    let registry = OriginRegistry::default();
    registry.remember("transcribe", Some(origin("a.exe")));
    assert_eq!(registry.take("transcribe").unwrap().exe_name, "a.exe");
    assert_eq!(registry.take("transcribe"), None, "taken once");

    // A queued second session cannot disturb the first one's taken value.
    registry.remember("transcribe", Some(origin("a.exe")));
    let first = registry.take("transcribe");
    registry.remember("transcribe", Some(origin("b.exe")));
    assert_eq!(first.unwrap().exe_name, "a.exe");
    assert_eq!(registry.take("transcribe").unwrap().exe_name, "b.exe");
}

#[test]
fn failed_probe_clears_the_stale_origin() {
    let registry = OriginRegistry::default();
    registry.remember("transcribe", Some(origin("a.exe")));
    registry.remember("transcribe", None);
    assert_eq!(registry.take("transcribe"), None);
}

#[test]
fn cancelled_capture_drops_origin_without_changing_a_queued_session() {
    let registry = OriginRegistry::default();
    registry.remember("transcribe", Some(origin("a.exe")));
    let queued = registry.take("transcribe");
    registry.remember("transcribe", Some(origin("b.exe")));
    registry.clear();
    assert_eq!(registry.take("transcribe"), None);
    assert_eq!(queued.unwrap().exe_name, "a.exe");
}

#[test]
fn bindings_do_not_share_origins() {
    let registry = OriginRegistry::default();
    registry.remember("transcribe", Some(origin("a.exe")));
    registry.remember("assistant", Some(origin("b.exe")));
    assert_eq!(registry.take("assistant").unwrap().exe_name, "b.exe");
    assert_eq!(registry.take("transcribe").unwrap().exe_name, "a.exe");
}

#[test]
fn global_remember_and_take_round_trip() {
    remember_foreground("origin-test-binding");
    // Off Windows (or without focus) nothing is recorded; either way a
    // second take is empty and nothing panics.
    let _ = take("origin-test-binding");
    assert_eq!(take("origin-test-binding"), None);
}
