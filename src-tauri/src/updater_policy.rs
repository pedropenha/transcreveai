//! Whether this build has a release channel the updater may talk to.
//!
//! The fork ships without an updater endpoint or signing key until it has its
//! own release pipeline (T-049): pointing at upstream Handy's feed would offer
//! Handy builds to our users. Deriving the answer from `tauri.conf.json` keeps
//! a single source of truth — adding our endpoint and key re-enables updates.

// Only the base config is read: an updater section added through a platform
// overlay (`tauri.windows.conf.json`) or a CI `--config` merge is not seen and
// the build is treated as having no channel (fail-safe). Put the channel here.
const TAURI_CONF: &str = include_str!("../tauri.conf.json");

/// True when `conf_json` configures at least one updater endpoint and a
/// public key to verify what those endpoints serve.
fn has_release_channel(conf_json: &str) -> bool {
    let Ok(conf) = serde_json::from_str::<serde_json::Value>(conf_json) else {
        return false;
    };
    let updater = &conf["plugins"]["updater"];
    let has_endpoint = updater["endpoints"]
        .as_array()
        .is_some_and(|endpoints| !endpoints.is_empty());
    let has_pubkey = updater["pubkey"]
        .as_str()
        .is_some_and(|key| !key.trim().is_empty());
    has_endpoint && has_pubkey
}

/// Release channel state of the running build (computed once).
pub fn build_has_release_channel() -> bool {
    use std::sync::OnceLock;
    static HAS_CHANNEL: OnceLock<bool> = OnceLock::new();
    *HAS_CHANNEL.get_or_init(|| has_release_channel(TAURI_CONF))
}

#[cfg(test)]
mod tests {
    use super::*;

    const WITH_CHANNEL: &str = r#"{"plugins":{"updater":{"pubkey":"abc","endpoints":["https://example.com/latest.json"]}}}"#;

    #[test]
    fn endpoint_and_pubkey_make_a_channel() {
        assert!(has_release_channel(WITH_CHANNEL));
    }

    #[test]
    fn empty_endpoints_are_not_a_channel() {
        let conf = r#"{"plugins":{"updater":{"pubkey":"abc","endpoints":[]}}}"#;
        assert!(!has_release_channel(conf));
    }

    #[test]
    fn blank_pubkey_is_not_a_channel() {
        let conf = r#"{"plugins":{"updater":{"pubkey":"  ","endpoints":["https://example.com/latest.json"]}}}"#;
        assert!(!has_release_channel(conf));
    }

    #[test]
    fn missing_updater_section_or_bad_json_is_not_a_channel() {
        assert!(!has_release_channel(r#"{"plugins":{}}"#));
        assert!(!has_release_channel("not json"));
    }

    #[test]
    fn shipped_config_never_points_at_upstream_handy() {
        assert!(
            !TAURI_CONF.contains("cjpais/Handy"),
            "tauri.conf.json must not use upstream Handy's updater feed"
        );
    }

    #[test]
    fn shipped_config_has_no_channel_until_t049() {
        assert!(
            !build_has_release_channel(),
            "remove this test when T-049 adds our own updater endpoint and key"
        );
    }
}
