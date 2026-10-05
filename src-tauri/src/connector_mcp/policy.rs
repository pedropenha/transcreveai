//! Shared fail-closed checks for every authenticated bridge request.

pub(super) fn secret_matches(expected: &str, supplied: &str) -> bool {
    !expected.is_empty() && expected == supplied
}

pub(super) fn grant_active(
    expected_client: &str,
    client: &str,
    expires: u64,
    now: u64,
    revoked: bool,
) -> bool {
    expected_client == client && !revoked && expires > now
}

pub(super) fn connection_allowed(scope: &[String], id: &str, enabled: bool, offline: bool) -> bool {
    !offline && enabled && scope.iter().any(|allowed| allowed == id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_comparison_rejects_wrong_length_and_content() {
        assert!(secret_matches("canary-grant", "canary-grant"));
        assert!(!secret_matches("canary-grant", "canary-grans"));
        assert!(!secret_matches("canary-grant", "canary-grant-extra"));
        assert!(!secret_matches("", ""));
    }

    #[test]
    fn grants_are_bound_to_client_expiry_and_revocation() {
        assert!(grant_active("a", "a", 101, 100, false));
        assert!(!grant_active("a", "b", 101, 100, false));
        assert!(!grant_active("a", "a", 100, 100, false));
        assert!(!grant_active("a", "a", 101, 100, true));
    }

    #[test]
    fn scope_rejects_other_connection_removed_disabled_and_offline() {
        let scope = ["allowed".to_owned()];
        assert!(connection_allowed(&scope, "allowed", true, false));
        assert!(!connection_allowed(&scope, "other", true, false));
        assert!(!connection_allowed(&scope, "allowed", false, false));
        assert!(!connection_allowed(&scope, "allowed", true, true));
    }
}
