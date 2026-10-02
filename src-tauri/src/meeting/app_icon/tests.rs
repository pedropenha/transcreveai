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
