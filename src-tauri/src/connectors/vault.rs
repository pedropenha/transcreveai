//! Versioned OAuth bundle in the existing OS vault. A final manifest write is
//! the commit point: an interrupted rotation cannot expose a partial bundle.
use super::types::*;
use crate::secrets::SecretStore;
use serde::{Deserialize, Serialize};
const CHUNK_BYTES: usize = 900;
const MAX_BUNDLE_BYTES: usize = 32 * 1024;
#[derive(Serialize, Deserialize)]
pub struct TokenBundle {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_at: u64,
    pub client_id: String,
    pub auth_url: String,
    pub token_url: String,
    pub resource: Option<String>,
    pub redirect_uri: String,
}
#[derive(Serialize, Deserialize)]
struct Manifest {
    generation: String,
    chunks: usize,
}
fn key(id: &str) -> ConnectorResult<String> {
    Ok(format!("connector-{}", super::policy::canonical_id(id)?))
}
fn failure() -> ConnectorError {
    ConnectorError::new(ConnectorErrorCode::VaultUnavailable)
}
fn manifest(store: &dyn SecretStore, prefix: &str) -> ConnectorResult<Option<Manifest>> {
    store
        .get(prefix)
        .map_err(|_| failure())?
        .map(|s| serde_json::from_str(&s).map_err(|_| failure()))
        .transpose()
}
pub fn load(store: &dyn SecretStore, id: &str) -> ConnectorResult<Option<TokenBundle>> {
    let prefix = key(id)?;
    let Some(m) = manifest(store, &prefix)? else {
        return Ok(None);
    };
    if m.chunks == 0 || m.chunks > 40 || uuid::Uuid::parse_str(&m.generation).is_err() {
        return Err(failure());
    }
    let mut json = String::new();
    for i in 0..m.chunks {
        let chunk = store
            .get(&format!("{prefix}-{}-{i}", m.generation))
            .map_err(|_| failure())?
            .ok_or_else(failure)?;
        if chunk.len() > CHUNK_BYTES || json.len() + chunk.len() > MAX_BUNDLE_BYTES {
            return Err(failure());
        }
        json.push_str(&chunk);
    }
    serde_json::from_str(&json).map(Some).map_err(|_| failure())
}
pub fn save(store: &dyn SecretStore, id: &str, bundle: &TokenBundle) -> ConnectorResult<()> {
    let prefix = key(id)?;
    let previous = manifest(store, &prefix)?;
    let json = serde_json::to_string(bundle).map_err(|_| failure())?;
    if json.len() > MAX_BUNDLE_BYTES {
        return Err(failure());
    }
    let generation = uuid::Uuid::new_v4().to_string();
    let mut chunks = Vec::new();
    let mut start = 0;
    while start < json.len() {
        let mut end = (start + CHUNK_BYTES).min(json.len());
        while !json.is_char_boundary(end) {
            end -= 1;
        }
        chunks.push(&json[start..end]);
        start = end;
    }
    for (i, chunk) in chunks.iter().enumerate() {
        if store
            .set(&format!("{prefix}-{generation}-{i}"), chunk)
            .is_err()
        {
            for j in 0..i {
                let _ = store.delete(&format!("{prefix}-{generation}-{j}"));
            }
            return Err(failure());
        }
    }
    let m = Manifest {
        generation: generation.clone(),
        chunks: chunks.len(),
    };
    let metadata = serde_json::to_string(&m).map_err(|_| failure())?;
    if store.set(&prefix, &metadata).is_err() {
        for i in 0..m.chunks {
            let _ = store.delete(&format!("{prefix}-{generation}-{i}"));
        }
        return Err(failure());
    }
    if let Some(old) = previous {
        erase_chunks(store, &prefix, &old);
    }
    Ok(())
}
fn erase_chunks(store: &dyn SecretStore, prefix: &str, m: &Manifest) {
    for i in 0..m.chunks.min(40) {
        let _ = store.delete(&format!("{prefix}-{}-{i}", m.generation));
    }
}
pub fn delete(store: &dyn SecretStore, id: &str) -> ConnectorResult<()> {
    let prefix = key(id)?;
    let m = manifest(store, &prefix)?;
    store.delete(&prefix).map_err(|_| failure())?;
    if let Some(m) = m {
        erase_chunks(store, &prefix, &m);
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::secrets::MemorySecretStore;
    #[test]
    fn bundle_longer_than_windows_blob_roundtrips_and_rotates() {
        let store = MemorySecretStore::default();
        let id = uuid::Uuid::new_v4().to_string();
        let mut b = TokenBundle {
            access_token: "canary".repeat(900),
            refresh_token: Some("refresh-canary".into()),
            expires_at: 100,
            client_id: "public".into(),
            auth_url: "https://example.com".into(),
            token_url: "https://example.com".into(),
            resource: None,
            redirect_uri: "http://127.0.0.1:1/oauth/callback".into(),
        };
        save(&store, &id, &b).unwrap();
        assert_eq!(
            load(&store, &id).unwrap().unwrap().access_token,
            b.access_token
        );
        b.access_token = "rotated".into();
        save(&store, &id, &b).unwrap();
        assert_eq!(load(&store, &id).unwrap().unwrap().access_token, "rotated");
        delete(&store, &id).unwrap();
        assert!(load(&store, &id).unwrap().is_none());
    }
}
