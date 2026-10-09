//! Meeting-app icon resolution for the detection toast (spec F008, FR-008-07).
//!
//! Priority (pure, [`extract_target`]):
//! 1. Known apps (Zoom, Teams, Meet, Webex, Discord, Slack) never extract —
//!    the front-end ships an embedded logo for them (Meet runs inside a
//!    browser; new Teams is packaged and has no exe path).
//! 2. Browsers never use the exe icon (it would mislabel a web meeting).
//! 3. Any other app with an exe path gets the executable's own icon, as a
//!    `data:image/png;base64,…` URI.
//! 4. Otherwise no icon.
//!
//! Extraction sits behind [`IconExtractor`] so the cache and the RGBA→PNG
//! encoding are testable without the Win32 API; results (including failures)
//! are cached per exe path in a bounded map.

use std::collections::HashMap;
use std::sync::Mutex;

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;

use super::classifier::is_browser_exe;

#[cfg(windows)]
mod win;
#[cfg(windows)]
pub(crate) use win::WindowsIconExtractor;

#[cfg(target_os = "macos")]
mod mac;
#[cfg(target_os = "macos")]
pub(crate) use mac::MacIconExtractor;

#[cfg(test)]
mod tests;

const PNG_DATA_URI_PREFIX: &str = "data:image/png;base64,";
/// Max distinct exe paths remembered. Worst case is
/// `CACHE_CAPACITY * MAX_DATA_URI_BYTES` = 4 MiB (typical icons are ~5 KB).
#[cfg(any(windows, target_os = "macos"))]
const CACHE_CAPACITY: usize = 64;
/// Hard ceiling on one data URI; larger icons are dropped (the toast shows
/// no icon rather than shipping a heavy IPC payload).
const MAX_DATA_URI_BYTES: usize = 64 * 1024;
/// Label/exe tokens whose logo is embedded in the front-end.
const KNOWN_APP_TOKENS: [&str; 6] = ["zoom", "teams", "meet", "webex", "discord", "slack"];

/// Decoded icon pixels, straight (non-premultiplied) RGBA8.
pub(crate) struct RgbaImage {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// Outcome of one extraction attempt.
pub(crate) enum Extraction {
    /// Windows extractor yields raw pixels, PNG-encoded by the caller.
    #[cfg_attr(not(any(windows, test)), allow(dead_code))]
    Found(RgbaImage),
    /// The extractor emits ready PNG bytes (macOS renders an
    /// `NSBitmapImageRep`); they go straight into the data URI.
    #[cfg_attr(not(any(target_os = "macos", test)), allow(dead_code))]
    FoundPng(Vec<u8>),
    /// This exe has no usable icon (missing, rejected path, unreadable
    /// bitmap): safe to remember.
    Unavailable,
}

/// Source of executable icons — the Win32 implementation or a test double.
pub(crate) trait IconExtractor {
    fn extract(&self, exe_path: &str) -> Extraction;
}

fn has_known_token(text: &str) -> bool {
    text.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .any(|token| KNOWN_APP_TOKENS.contains(&token))
}

/// Whether the front-end has an embedded logo for this app. Only the label
/// (what the front-end sees) counts: the exe name alone must not hide an icon.
fn is_known_app(label: &str) -> bool {
    has_known_token(label)
}

/// `exe_path` comes from the registry (untrusted): accept only a local
/// absolute `X:\...\*.exe`. UNC, `\?\`, `\.\` and any `\`/`//` prefix are
/// rejected so the shell is never pointed at a network share or device.
pub(crate) fn is_safe_exe_path(path: &str) -> bool {
    let bytes = path.as_bytes();
    let drive_form =
        bytes.len() > 3 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' && bytes[2] == b'\\';
    let exe = path.len() > 4
        && path
            .get(path.len() - 4..)
            .is_some_and(|ext| ext.eq_ignore_ascii_case(".exe"));
    // No NUL, forward slashes, NTFS alternate streams (`:` past the drive
    // letter) or `..` segments.
    let clean = !path.contains('\0')
        && !path.contains('/')
        && !path.get(2..).is_some_and(|rest| rest.contains(':'))
        && !path.split('\\').any(|segment| segment == "..");
    drive_form && exe && clean
}

/// macOS icon source: a `.app` bundle path. Absolute POSIX path with the
/// same no-escape rules as [`is_safe_exe_path`] — no NUL, no backslashes,
/// no `..` segments.
fn is_safe_app_bundle_path(path: &str) -> bool {
    path.starts_with('/')
        && path.ends_with(".app")
        && !path.contains('\0')
        && !path.contains('\\')
        && !path.split('/').any(|segment| segment == "..")
}

/// Icon source path in either platform shape: `X:\…\*.exe` on Windows,
/// `/…/*.app` on macOS. Both pass through the same persistence field.
pub(crate) fn is_safe_icon_source_path(path: &str) -> bool {
    is_safe_exe_path(path) || is_safe_app_bundle_path(path)
}

/// Longest exe path persisted (Windows `MAX_PATH` is 260; long-path
/// installs stay well below this).
const MAX_EXE_PATH_CHARS: usize = 520;

/// Boundary gate for an exe path arriving over IPC before it is stored on a
/// meeting: trimmed, bounded and [`is_safe_icon_source_path`]-safe, else dropped.
pub(crate) fn sanitize_exe_path(raw: Option<String>) -> Option<String> {
    raw.map(|path| path.trim().to_string())
        .filter(|path| path.chars().count() <= MAX_EXE_PATH_CHARS && is_safe_icon_source_path(path))
}

/// The exe path whose icon should be extracted, or `None` when the app has
/// an embedded logo (by label), is a browser, or has no safe path.
pub(crate) fn extract_target<'a>(
    label: &str,
    exe_name: &str,
    exe_path: Option<&'a str>,
) -> Option<&'a str> {
    if is_known_app(label) || is_browser_exe(exe_name) {
        return None;
    }
    exe_path.filter(|path| is_safe_icon_source_path(path))
}

/// Encode straight RGBA8 pixels as a PNG data URI; `None` on a bad buffer or
/// when the URI would exceed [`MAX_DATA_URI_BYTES`].
pub(crate) fn rgba_to_png_data_uri(width: u32, height: u32, rgba: &[u8]) -> Option<String> {
    let expected = (width as usize)
        .checked_mul(height as usize)?
        .checked_mul(4)?;
    if width == 0 || height == 0 || rgba.len() != expected {
        return None;
    }
    let mut out = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut out, width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().ok()?;
        writer.write_image_data(rgba).ok()?;
    }
    let uri = format!("{PNG_DATA_URI_PREFIX}{}", BASE64.encode(out));
    (uri.len() <= MAX_DATA_URI_BYTES).then_some(uri)
}

/// Bounded per-exe-path cache of icon data URIs (permanent failures cached as
/// `None`; transient ones are not cached).
pub(crate) struct IconCache<E: IconExtractor> {
    extractor: E,
    entries: Mutex<HashMap<String, Option<String>>>,
    capacity: usize,
}

impl<E: IconExtractor> IconCache<E> {
    pub(crate) fn new(extractor: E, capacity: usize) -> Self {
        Self {
            extractor,
            entries: Mutex::new(HashMap::new()),
            capacity: capacity.max(1),
        }
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.lock().len()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Option<String>>> {
        self.entries.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Cached data URI for `exe_path`, extracting on first use. The lock is
    /// not held while extracting.
    pub(crate) fn resolve(&self, exe_path: &str) -> Option<String> {
        let key = exe_path.to_lowercase();
        if let Some(hit) = self.lock().get(&key) {
            return hit.clone();
        }
        let icon = match self.extractor.extract(exe_path) {
            Extraction::Found(img) => rgba_to_png_data_uri(img.width, img.height, &img.rgba),
            Extraction::FoundPng(png) => png_to_data_uri(&png),
            Extraction::Unavailable => None,
        };
        let mut entries = self.lock();
        if entries.len() >= self.capacity {
            entries.clear();
        }
        entries.insert(key, icon.clone());
        icon
    }
}

/// Icon for a detection: applies the priority rule, then the cache.
pub(crate) fn icon_for<E: IconExtractor>(
    cache: &IconCache<E>,
    label: &str,
    exe_name: &str,
    exe_path: Option<&str>,
) -> Option<String> {
    extract_target(label, exe_name, exe_path).and_then(|path| cache.resolve(path))
}

/// The exe path whose icon a *dictation* shows (F010 history). Unlike
/// [`extract_target`], browsers are allowed: the app label of a dictation is
/// the browser itself ("Chrome"), so its own icon is the right one. Apps with
/// an embedded front-end logo (by label) still never extract.
pub(crate) fn dictation_extract_target<'a>(
    label: &str,
    exe_path: Option<&'a str>,
) -> Option<&'a str> {
    if is_known_app(label) {
        return None;
    }
    exe_path.filter(|path| is_safe_icon_source_path(path))
}

/// Encode ready PNG bytes as a data URI; `None` when the buffer isn't a PNG
/// or the URI would exceed [`MAX_DATA_URI_BYTES`].
pub(crate) fn png_to_data_uri(png: &[u8]) -> Option<String> {
    const PNG_MAGIC: &[u8; 8] = b"\x89PNG\r\n\x1a\n";
    if !png.starts_with(PNG_MAGIC) {
        return None;
    }
    let uri = format!("{PNG_DATA_URI_PREFIX}{}", BASE64.encode(png));
    (uri.len() <= MAX_DATA_URI_BYTES).then_some(uri)
}

/// Icon for a dictation's origin app: priority rule, then the cache.
pub(crate) fn dictation_icon_for<E: IconExtractor>(
    cache: &IconCache<E>,
    label: &str,
    exe_path: Option<&str>,
) -> Option<String> {
    dictation_extract_target(label, exe_path).and_then(|path| cache.resolve(path))
}

#[cfg(windows)]
fn shared_cache() -> &'static IconCache<WindowsIconExtractor> {
    static CACHE: std::sync::OnceLock<IconCache<WindowsIconExtractor>> = std::sync::OnceLock::new();
    CACHE.get_or_init(|| IconCache::new(WindowsIconExtractor, CACHE_CAPACITY))
}

/// Process-wide icon for a detection (Windows only).
#[cfg(windows)]
pub(crate) fn detection_icon(
    label: &str,
    exe_name: &str,
    exe_path: Option<&str>,
) -> Option<String> {
    icon_for(shared_cache(), label, exe_name, exe_path)
}

/// Process-wide icon for a dictation's origin app (Windows only); shares the
/// bounded cache with [`detection_icon`].
#[cfg(windows)]
pub(crate) fn dictation_icon(label: &str, exe_path: Option<&str>) -> Option<String> {
    dictation_icon_for(shared_cache(), label, exe_path)
}

#[cfg(target_os = "macos")]
fn shared_cache() -> &'static IconCache<MacIconExtractor> {
    static CACHE: std::sync::OnceLock<IconCache<MacIconExtractor>> = std::sync::OnceLock::new();
    CACHE.get_or_init(|| IconCache::new(MacIconExtractor, CACHE_CAPACITY))
}

/// Process-wide icon for a detection (macOS): `NSWorkspace.iconForFile` on
/// the `.app` bundle, rendered to PNG.
#[cfg(target_os = "macos")]
pub(crate) fn detection_icon(
    label: &str,
    exe_name: &str,
    exe_path: Option<&str>,
) -> Option<String> {
    icon_for(shared_cache(), label, exe_name, exe_path)
}

/// Process-wide icon for a dictation's origin app (macOS); shares the
/// bounded cache with [`detection_icon`].
#[cfg(target_os = "macos")]
pub(crate) fn dictation_icon(label: &str, exe_path: Option<&str>) -> Option<String> {
    dictation_icon_for(shared_cache(), label, exe_path)
}
