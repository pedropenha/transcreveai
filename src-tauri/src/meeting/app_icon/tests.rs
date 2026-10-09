use std::sync::atomic::{AtomicUsize, Ordering};

use super::*;

fn px(n: usize) -> Vec<u8> {
    vec![200; n * 4]
}

#[test]
fn encodes_rgba_as_png_data_uri() {
    let uri = rgba_to_png_data_uri(2, 2, &px(4)).unwrap();
    let b64 = uri.strip_prefix("data:image/png;base64,").unwrap();
    let bytes = BASE64.decode(b64).unwrap();
    assert_eq!(&bytes[..8], b"\x89PNG\r\n\x1a\n");
}

#[test]
fn rejects_buffer_with_wrong_length() {
    assert!(rgba_to_png_data_uri(2, 2, &px(3)).is_none());
}

#[test]
fn rejects_zero_dimensions() {
    assert!(rgba_to_png_data_uri(0, 4, &[]).is_none());
}

#[test]
fn known_apps_use_embedded_logo() {
    for label in [
        "Zoom",
        "Microsoft Teams",
        "Google Meet",
        "Webex",
        "Discord",
        "slack",
    ] {
        assert_eq!(
            extract_target(label, "x.exe", Some("C:\\x.exe")),
            None,
            "{label}"
        );
    }
}

#[test]
fn exe_name_alone_does_not_hide_extracted_icon() {
    assert_eq!(
        extract_target("Reuniao", "Zoom.exe", Some("C:\\z.exe")),
        Some("C:\\z.exe")
    );
    assert_eq!(extract_target("ms-teams", "ms-teams.exe", None), None);
}

#[test]
fn browsers_never_use_exe_icon() {
    assert_eq!(
        extract_target("Reuniao X", "chrome.exe", Some("C:\\chrome.exe")),
        None
    );
}

#[test]
fn unknown_app_with_path_is_extracted() {
    assert_eq!(
        extract_target("Audacity", "audacity.exe", Some("C:\\a\\audacity.exe")),
        Some("C:\\a\\audacity.exe")
    );
}

#[test]
fn unknown_app_without_path_has_no_icon() {
    assert_eq!(extract_target("Audacity", "audacity.exe", None), None);
}

#[test]
fn lookalike_label_is_not_a_known_app() {
    assert_eq!(
        extract_target("Meeting Pro", "mp.exe", Some("C:\\mp.exe")),
        Some("C:\\mp.exe")
    );
}

struct Counting(AtomicUsize, bool);

impl IconExtractor for Counting {
    fn extract(&self, _path: &str) -> Extraction {
        self.0.fetch_add(1, Ordering::SeqCst);
        if self.1 {
            Extraction::Found(RgbaImage {
                width: 2,
                height: 2,
                rgba: px(4),
            })
        } else {
            Extraction::Unavailable
        }
    }
}

#[test]
fn cache_extracts_once_per_path_case_insensitively() {
    let cache = IconCache::new(Counting(AtomicUsize::new(0), true), 8);
    let a = cache.resolve("C:\\A.exe");
    let b = cache.resolve("c:\\a.exe");
    assert!(a.is_some());
    assert_eq!(a, b);
    assert_eq!(cache.extractor.0.load(Ordering::SeqCst), 1);
}

#[test]
fn cache_remembers_failures() {
    let cache = IconCache::new(Counting(AtomicUsize::new(0), false), 8);
    assert!(cache.resolve("C:\\a.exe").is_none());
    assert!(cache.resolve("C:\\a.exe").is_none());
    assert_eq!(cache.extractor.0.load(Ordering::SeqCst), 1);
}

#[test]
fn cache_is_bounded() {
    let cache = IconCache::new(Counting(AtomicUsize::new(0), true), 2);
    for i in 0..5 {
        cache.resolve(&format!("C:\\{i}.exe"));
    }
    assert!(cache.len() <= 2);
}

#[test]
fn icon_for_applies_priority_then_cache() {
    let cache = IconCache::new(Counting(AtomicUsize::new(0), true), 8);
    assert!(icon_for(&cache, "Zoom", "zoom.exe", Some("C:\\zoom.exe")).is_none());
    assert!(icon_for(&cache, "Foo", "foo.exe", Some("C:\\foo.exe")).is_some());
    assert_eq!(cache.extractor.0.load(Ordering::SeqCst), 1);
}

#[test]
fn sanitize_exe_path_keeps_only_safe_bounded_paths() {
    assert_eq!(
        sanitize_exe_path(Some("  C:\\apps\\Zoom.exe ".to_string())).as_deref(),
        Some("C:\\apps\\Zoom.exe")
    );
    for bad in [
        "\\\\server\\share\\x.exe",
        "\\\\?\\C:\\x.exe",
        "C:\\apps\\readme.txt",
        "relative\\x.exe",
        "C:\\a\\..\\x.exe",
        "C:\\f.txt:evil.exe",
        "C:/apps/x.exe",
        "",
    ] {
        assert_eq!(sanitize_exe_path(Some(bad.to_string())), None, "{bad:?}");
    }
    assert_eq!(sanitize_exe_path(None), None);
    let long = format!("C:\\{}\\x.exe", "a".repeat(600));
    assert_eq!(sanitize_exe_path(Some(long)), None, "over-long path");
}

#[test]
fn dictation_icon_allows_browsers_but_not_embedded_apps() {
    // Browser label is the browser itself: its own exe icon is right.
    assert_eq!(
        dictation_extract_target("Chrome", Some("C:\\g\\chrome.exe")),
        Some("C:\\g\\chrome.exe")
    );
    // Embedded front-end logos win over extraction.
    for label in ["Zoom", "Teams", "Slack", "Discord"] {
        assert_eq!(
            dictation_extract_target(label, Some("C:\\x.exe")),
            None,
            "{label}"
        );
    }
    // Claude is not embedded -> extracted.
    assert_eq!(
        dictation_extract_target("Claude", Some("C:\\a\\Claude.exe")),
        Some("C:\\a\\Claude.exe")
    );
}

#[test]
fn dictation_icon_needs_a_safe_path() {
    assert_eq!(dictation_extract_target("Claude", None), None);
    assert_eq!(
        dictation_extract_target("Claude", Some("\\\\server\\share\\Claude.exe")),
        None
    );
    assert_eq!(
        dictation_extract_target("Claude", Some("C:\\a\\..\\Claude.exe")),
        None
    );
}

#[test]
fn dictation_icon_for_uses_the_shared_cache_once_per_path() {
    let cache = IconCache::new(Counting(AtomicUsize::new(0), true), 8);
    assert!(dictation_icon_for(&cache, "Chrome", Some("C:\\g\\chrome.exe")).is_some());
    assert!(dictation_icon_for(&cache, "Chrome", Some("C:\\g\\chrome.exe")).is_some());
    assert!(dictation_icon_for(&cache, "Zoom", Some("C:\\g\\zoom.exe")).is_none());
    assert_eq!(cache.extractor.0.load(Ordering::SeqCst), 1);
}

#[test]
fn sanitize_accepts_macos_app_bundle_paths() {
    assert_eq!(
        sanitize_exe_path(Some("  /Applications/Google Chrome.app ".to_string())).as_deref(),
        Some("/Applications/Google Chrome.app")
    );
    for bad in [
        "relative/Foo.app",
        "/Applications/Foo.app/Contents/MacOS/Foo",
        "/Applications/x.txt",
        "/Applications/../evil.app",
        "/Applications/back\\slash.app",
        "",
    ] {
        assert_eq!(sanitize_exe_path(Some(bad.to_string())), None, "{bad:?}");
    }
}

#[test]
fn png_to_data_uri_validates_magic_and_size() {
    let uri = rgba_to_png_data_uri(2, 2, &px(4)).unwrap();
    let bytes = BASE64
        .decode(uri.strip_prefix("data:image/png;base64,").unwrap())
        .unwrap();
    assert_eq!(png_to_data_uri(&bytes).as_deref(), Some(uri.as_str()));
    assert!(png_to_data_uri(b"not a png").is_none());
}

struct ReadyPng;

impl IconExtractor for ReadyPng {
    fn extract(&self, _path: &str) -> Extraction {
        let uri = rgba_to_png_data_uri(2, 2, &px(4)).unwrap();
        let bytes = BASE64
            .decode(uri.strip_prefix("data:image/png;base64,").unwrap())
            .unwrap();
        Extraction::FoundPng(bytes)
    }
}

#[test]
fn cache_resolves_ready_png_bytes() {
    let cache = IconCache::new(ReadyPng, 8);
    let uri = cache.resolve("/Applications/Foo.app").expect("icon");
    assert!(uri.starts_with("data:image/png;base64,"));
}

#[cfg(target_os = "macos")]
#[test]
fn extracts_real_icon_from_finder() {
    let Extraction::FoundPng(bytes) =
        MacIconExtractor.extract("/System/Library/CoreServices/Finder.app")
    else {
        panic!("finder icon");
    };
    let uri = png_to_data_uri(&bytes).expect("png");
    assert!(uri.starts_with("data:image/png;base64,"));
    assert!(uri.len() > "data:image/png;base64,".len() + 100);
}

#[cfg(windows)]
#[test]
fn dictation_icon_extracts_real_notepad_icon() {
    // The shell can miss its 2 s budget while the whole suite hammers it in
    // parallel; a fresh cache per attempt keeps a cached miss from sticking.
    let uri = (0..3)
        .find_map(|_| {
            let cache = IconCache::new(WindowsIconExtractor, 4);
            dictation_icon_for(
                &cache,
                "Bloco de Notas",
                Some("C:\\Windows\\System32\\notepad.exe"),
            )
        })
        .expect("notepad icon");
    assert!(uri.starts_with("data:image/png;base64,"));
}

/// Real-machine probe: set `TRANSCREVE_PROBE_EXE` to any exe path (e.g. a
/// packaged app under the restricted `WindowsApps` ACL) to see whether its
/// icon extracts. Reports instead of asserting — the outcome depends on the
/// machine, and the UI falls back to a monogram either way.
#[cfg(windows)]
#[test]
fn reports_icon_extraction_for_probe_exe() {
    let Ok(path) = std::env::var("TRANSCREVE_PROBE_EXE") else {
        return;
    };
    let target = dictation_extract_target("probe", Some(&path));
    println!(
        "PROBE-ICON: path={path} safe={} extracted={}",
        target.is_some(),
        extraction_succeeds(&path)
    );
}

#[cfg(windows)]
fn extraction_succeeds(path: &str) -> bool {
    matches!(WindowsIconExtractor.extract(path), Extraction::Found(_))
}

#[cfg(windows)]
#[test]
fn extracts_real_icon_from_notepad() {
    let path = "C:\\Windows\\System32\\notepad.exe";
    let Extraction::Found(img) = WindowsIconExtractor.extract(path) else {
        panic!("icon");
    };
    let uri = rgba_to_png_data_uri(img.width, img.height, &img.rgba).expect("icon");
    assert!(uri.starts_with("data:image/png;base64,"));
    assert!(uri.len() > "data:image/png;base64,".len() + 100);
}

#[cfg(windows)]
#[test]
fn missing_exe_yields_none() {
    assert!(matches!(
        WindowsIconExtractor.extract("C:\\definitely\\missing\\x.exe"),
        Extraction::Unavailable
    ));
}
