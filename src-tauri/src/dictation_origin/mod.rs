//! Origin app of a dictation session (F010 history, F002 pipeline).
//!
//! The foreground app is read when the session *starts* — the Flow Bar is
//! non-focusable, so focus is still where the user is dictating — and rides
//! with the session until the pipeline saves its history row. Sessions can be
//! queued FIFO and the pipeline runs after `stop`, so the value is parked per
//! binding at `start` and *taken* (moved out) synchronously at `stop`: a later
//! session can never overwrite the origin an earlier one still needs.
//!
//! Persisted as `app_exe` (file name, e.g. `Claude.exe`), `app_name`
//! (friendly label) and `app_exe_path` (sanitized full path, only used to
//! extract the icon and never sent over IPC).

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use crate::meeting::app_icon::sanitize_exe_path;

#[cfg(windows)]
mod win;

/// Where one dictation was spoken.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DictationOrigin {
    /// Executable file name, original case (`Claude.exe`).
    pub exe_name: String,
    /// Friendly label shown in history (`Claude`, `Chrome`).
    pub app_name: String,
    /// Sanitized full path (`is_safe_exe_path`), `None` when unsafe/unknown.
    pub exe_path: Option<String>,
}

/// Friendly labels for common apps, keyed by lower-cased exe stem.
const FRIENDLY_NAMES: &[(&str, &str)] = &[
    ("claude", "Claude"),
    ("chrome", "Chrome"),
    ("msedge", "Edge"),
    ("firefox", "Firefox"),
    ("brave", "Brave"),
    ("opera", "Opera"),
    ("vivaldi", "Vivaldi"),
    ("code", "Visual Studio Code"),
    ("code - insiders", "Visual Studio Code"),
    ("cursor", "Cursor"),
    ("devenv", "Visual Studio"),
    ("slack", "Slack"),
    ("discord", "Discord"),
    ("ms-teams", "Teams"),
    ("msteams", "Teams"),
    ("teams", "Teams"),
    ("zoom", "Zoom"),
    ("notepad", "Bloco de Notas"),
    ("notepad++", "Notepad++"),
    ("windowsterminal", "Terminal"),
    ("wt", "Terminal"),
    ("powershell", "PowerShell"),
    ("pwsh", "PowerShell"),
    ("cmd", "Prompt de Comando"),
    ("outlook", "Outlook"),
    ("winword", "Word"),
    ("excel", "Excel"),
    ("powerpnt", "PowerPoint"),
    ("onenote", "OneNote"),
    ("explorer", "Explorador de Arquivos"),
    ("whatsapp", "WhatsApp"),
    ("telegram", "Telegram"),
    ("spotify", "Spotify"),
    ("notion", "Notion"),
    ("obsidian", "Obsidian"),
    ("transcreve-ai", "Transcreve.ai"),
];

/// Last path segment, accepting both separators.
fn file_name_of(path: &str) -> &str {
    path.rsplit(['\\', '/']).next().unwrap_or(path)
}

fn exe_stem(exe: &str) -> &str {
    let name = file_name_of(exe.trim());
    match name.len().checked_sub(4).and_then(|at| name.get(at..)) {
        Some(ext) if ext.eq_ignore_ascii_case(".exe") => &name[..name.len() - 4],
        _ => name,
    }
}

/// Human label for an executable: the curated map first, else the stem with
/// its first letter capitalized. Empty input gives an empty string.
pub fn friendly_app_name(exe: &str) -> String {
    let stem = exe_stem(exe);
    let lowered = stem.to_lowercase();
    if let Some((_, friendly)) = FRIENDLY_NAMES.iter().find(|(key, _)| *key == lowered) {
        return (*friendly).to_string();
    }
    let mut chars = stem.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

impl DictationOrigin {
    /// Origin from the foreground process image path. `None` when the path has
    /// no usable file name; an unsafe path still yields an origin, just
    /// without `exe_path` (no icon, the name is still worth keeping).
    pub fn from_image_path(image_path: &str) -> Option<Self> {
        let trimmed = image_path.trim();
        let exe_name = file_name_of(trimmed).trim();
        let app_name = friendly_app_name(exe_name);
        if exe_name.is_empty() || app_name.is_empty() {
            return None;
        }
        Some(Self {
            exe_name: exe_name.to_string(),
            app_name,
            exe_path: sanitize_exe_path(Some(trimmed.to_string())),
        })
    }
}

/// Origin parked per binding between `start` and `stop`.
#[derive(Default)]
struct OriginRegistry {
    parked: Mutex<HashMap<String, DictationOrigin>>,
}

impl OriginRegistry {
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, DictationOrigin>> {
        self.parked.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Park `origin` for `binding_id`; `None` clears a stale one so a session
    /// that could not probe never inherits the previous session's app.
    fn remember(&self, binding_id: &str, origin: Option<DictationOrigin>) {
        let mut parked = self.lock();
        match origin {
            Some(origin) => {
                parked.insert(binding_id.to_string(), origin);
            }
            None => {
                parked.remove(binding_id);
            }
        }
    }

    /// Move the parked origin out (the session owns it from here on).
    fn take(&self, binding_id: &str) -> Option<DictationOrigin> {
        self.lock().remove(binding_id)
    }

    fn clear(&self) {
        self.lock().clear();
    }
}

fn registry() -> &'static OriginRegistry {
    static REGISTRY: OnceLock<OriginRegistry> = OnceLock::new();
    REGISTRY.get_or_init(OriginRegistry::default)
}

/// Foreground app right now, `None` where it cannot be probed.
fn probe_foreground() -> Option<DictationOrigin> {
    #[cfg(windows)]
    {
        win::foreground_image_path().and_then(|path| DictationOrigin::from_image_path(&path))
    }
    #[cfg(not(windows))]
    {
        None
    }
}

/// Session start: record the foreground app for `binding_id`.
pub fn remember_foreground(binding_id: &str) {
    registry().remember(binding_id, probe_foreground());
}

/// Session stop: take the origin recorded at start (once).
pub fn take(binding_id: &str) -> Option<DictationOrigin> {
    registry().take(binding_id)
}

/// Drop the active capture's origin on cancellation or microphone failure.
/// Queued pipelines already took ownership at stop and remain unaffected.
pub fn clear() {
    registry().clear();
}

#[cfg(test)]
mod tests;
