//! Typed command errors for the IPC contract
//! (`specs/architecture/contracts.md` §5).
//!
//! Every Tauri command returns `Result<T, CommandError>` so the
//! tauri-specta envelope seen by the frontend is always
//! `{ status: "ok", data: T }` or
//! `{ status: "error", error: { code, message } }`. `code` is a stable,
//! machine-readable identifier the frontend can switch on (e.g.
//! `secure_input_active`); `message` is user-facing and must never contain
//! paths, stack traces, or secrets — the underlying detail goes to the log
//! via [`CommandError::logged`] (`rules/rust/security.md`).

use serde::Serialize;
use specta::Type;
use std::fmt;

/// Shorthand for the result type every IPC command returns.
pub type CommandResult<T> = Result<T, CommandError>;

/// Stable, machine-readable error codes for the IPC envelope's `code`
/// field. Serialized in `snake_case` (e.g. `secure_input_active`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum CommandErrorCode {
    /// Catch-all for failures that do not fit a more specific code.
    Internal,
    /// A referenced entity (provider, model, binding, prompt, history entry)
    /// does not exist.
    NotFound,
    /// Input rejected by validation at the system boundary.
    InvalidInput,
    /// Operation unsupported on this platform, build, or configuration.
    Unsupported,
    /// Missing an OS-level permission (accessibility, microphone, ...).
    PermissionDenied,
    /// A mutually exclusive operation is already in progress.
    Busy,
    /// macOS Secure Input is active; the frontend maps this marker to a
    /// localized explanation.
    SecureInputActive,
    /// An API key is required but none is configured.
    MissingApiKey,
    /// Audio device/stream failures.
    AudioDevice,
    /// Model download/load/unload/delete failures.
    Model,
    /// OS credential vault (keyring) failures.
    Keyring,
    /// External provider (HTTP) request failures.
    Provider,
    /// The meeting consent (FR-009-02) has not been acknowledged yet — the
    /// frontend shows the first-use modal in response to this code.
    ConsentRequired,
}

/// The error half of the IPC envelope — a stable `code` plus a
/// user-friendly `message` (no paths, stack traces, or secrets).
#[derive(Debug, Clone, Serialize, Type)]
pub struct CommandError {
    pub code: CommandErrorCode,
    pub message: String,
}

impl CommandError {
    /// Build an error whose `message` is already safe to show the user.
    pub fn new(code: CommandErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    /// Log the underlying `detail` and surface only the user-facing
    /// `message`, keeping internals out of the IPC payload.
    pub fn logged(
        code: CommandErrorCode,
        message: impl Into<String>,
        detail: impl fmt::Display,
    ) -> Self {
        let message = message.into();
        log::error!("{message}: {detail}");
        Self::new(code, message)
    }
}

impl fmt::Display for CommandError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for CommandError {}

/// Bridge for internal helpers that still surface `String` errors. The
/// string is used verbatim as the message, so only rely on it where the
/// helper is known to produce user-facing text.
impl From<String> for CommandError {
    fn from(message: String) -> Self {
        Self::new(CommandErrorCode::Internal, message)
    }
}

impl From<&str> for CommandError {
    fn from(message: &str) -> Self {
        Self::from(message.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_serializes_with_code_and_message() {
        let err = CommandError::new(CommandErrorCode::NotFound, "Model not found");
        let json = serde_json::to_value(&err).expect("CommandError must serialize");
        assert_eq!(
            json,
            serde_json::json!({
                "code": "not_found",
                "message": "Model not found",
            })
        );
    }

    #[test]
    fn every_code_serializes_to_snake_case() {
        for (code, expected) in [
            (CommandErrorCode::Internal, "internal"),
            (CommandErrorCode::NotFound, "not_found"),
            (CommandErrorCode::InvalidInput, "invalid_input"),
            (CommandErrorCode::Unsupported, "unsupported"),
            (CommandErrorCode::PermissionDenied, "permission_denied"),
            (CommandErrorCode::Busy, "busy"),
            (CommandErrorCode::SecureInputActive, "secure_input_active"),
            (CommandErrorCode::MissingApiKey, "missing_api_key"),
            (CommandErrorCode::AudioDevice, "audio_device"),
            (CommandErrorCode::Model, "model"),
            (CommandErrorCode::Keyring, "keyring"),
            (CommandErrorCode::Provider, "provider"),
            (CommandErrorCode::ConsentRequired, "consent_required"),
        ] {
            let json = serde_json::to_value(code).expect("code must serialize");
            assert_eq!(json, serde_json::json!(expected));
        }
    }

    #[test]
    fn logged_keeps_detail_out_of_the_message() {
        let err = CommandError::logged(
            CommandErrorCode::Internal,
            "Failed to open folder",
            "permission denied at /private/system/path",
        );
        assert_eq!(err.code, CommandErrorCode::Internal);
        assert_eq!(err.message, "Failed to open folder");
        assert!(!err.message.contains("/private/system/path"));
    }

    #[test]
    fn string_errors_bridge_to_internal_code() {
        let err: CommandError = "plain message".to_string().into();
        assert_eq!(err.code, CommandErrorCode::Internal);
        assert_eq!(err.message, "plain message");
    }
}
