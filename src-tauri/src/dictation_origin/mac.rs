//! macOS foreground-app probe: `NSWorkspace.frontmostApplication` yields the
//! owning `NSRunningApplication` — no Accessibility permission required.
//! The origin is identified by the app's `.app` bundle path, which is also
//! the input `NSWorkspace.iconForFile` needs for the history icon.

use objc2_app_kit::NSWorkspace;

use super::{file_name_of, DictationOrigin};
use crate::meeting::app_icon::sanitize_exe_path;

/// Foreground app right now: `exe_name` is the bundle file name
/// (`Google Chrome.app`), `app_name` the localized display name the Dock
/// shows, `exe_path` the `.app` bundle path. `None` when nothing is
/// frontmost or the app reports no name — e.g. under `cargo test` this
/// process isn't a bundled app, so there may be no answer at all.
pub(super) fn foreground() -> Option<DictationOrigin> {
    let app = NSWorkspace::sharedWorkspace().frontmostApplication()?;
    let app_name = app.localizedName()?.to_string();
    if app_name.trim().is_empty() {
        return None;
    }
    let bundle_path = app
        .bundleURL()
        .and_then(|url| url.path())
        .map(|path| path.to_string());
    let exe_name = bundle_path
        .as_deref()
        .map(|path| file_name_of(path).to_string())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| format!("{app_name}.app"));
    Some(DictationOrigin {
        exe_name,
        app_name,
        exe_path: sanitize_exe_path(bundle_path),
    })
}
