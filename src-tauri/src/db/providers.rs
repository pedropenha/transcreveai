//! Repository for the `providers` table (`data-model.md` §2): configured
//! STT/LLM providers. API keys never live here — `has_secret`/`secret_hint`
//! only describe the entry stored in the OS keychain
//! (`Transcreve.ai/provider/<provider_id>`).

use anyhow::Result;
use chrono::Utc;
use rusqlite::{params, Connection, OptionalExtension, Row};
use uuid::Uuid;

/// 'stt' | 'llm'
pub const KIND_STT: &str = "stt";
pub const KIND_LLM: &str = "llm";

#[derive(Clone, Debug, PartialEq)]
pub struct Provider {
    /// uuid
    pub id: String,
    /// 'stt' | 'llm'
    pub kind: String,
    /// local_whisper | local_parakeet | openai | groq | deepgram |
    /// openai_compat | anthropic | ollama
    pub provider_type: String,
    /// User-facing label.
    pub name: String,
    /// For openai_compat/ollama.
    pub base_url: Option<String>,
    pub model: String,
    /// JSON: temperature, language, timeouts…
    pub options_json: String,
    /// Whether a key is stored in the OS keychain.
    pub has_secret: bool,
    /// Last 4 chars of the key, display only.
    pub secret_hint: Option<String>,
    pub enabled: bool,
    /// Unix epoch seconds.
    pub created_at: i64,
    pub updated_at: i64,
}

impl Provider {
    /// Build a new provider with a fresh uuid and current timestamps.
    /// `provider_type`/`kind`/`name`/`model` are required by the schema.
    pub fn new(kind: &str, provider_type: &str, name: &str, model: &str) -> Self {
        let now = Utc::now().timestamp();
        Self {
            id: Uuid::new_v4().to_string(),
            kind: kind.to_string(),
            provider_type: provider_type.to_string(),
            name: name.to_string(),
            base_url: None,
            model: model.to_string(),
            options_json: "{}".to_string(),
            has_secret: false,
            secret_hint: None,
            enabled: true,
            created_at: now,
            updated_at: now,
        }
    }

    fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get("id")?,
            kind: row.get("kind")?,
            provider_type: row.get("type")?,
            name: row.get("name")?,
            base_url: row.get("base_url")?,
            model: row.get("model")?,
            options_json: row.get("options_json")?,
            has_secret: row.get("has_secret")?,
            secret_hint: row.get("secret_hint")?,
            enabled: row.get("enabled")?,
            created_at: row.get("created_at")?,
            updated_at: row.get("updated_at")?,
        })
    }
}

pub trait ProviderRepository {
    /// Insert or replace a provider; `updated_at` is refreshed on conflict.
    fn save(&self, provider: &Provider) -> Result<()>;
    fn get(&self, id: &str) -> Result<Option<Provider>>;
    /// All providers, or only those of `kind` ('stt'/'llm') when given.
    fn list(&self, kind: Option<&str>) -> Result<Vec<Provider>>;
    fn set_enabled(&self, id: &str, enabled: bool) -> Result<()>;
    fn delete(&self, id: &str) -> Result<()>;
}

pub struct SqliteProviderRepository<'a> {
    conn: &'a Connection,
}

impl<'a> SqliteProviderRepository<'a> {
    pub fn new(conn: &'a Connection) -> Self {
        Self { conn }
    }
}

impl ProviderRepository for SqliteProviderRepository<'_> {
    fn save(&self, provider: &Provider) -> Result<()> {
        self.conn.execute(
            "INSERT INTO providers (
                id, kind, type, name, base_url, model, options_json,
                has_secret, secret_hint, enabled, created_at, updated_at
            ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)
            ON CONFLICT(id) DO UPDATE SET
                kind = excluded.kind,
                type = excluded.type,
                name = excluded.name,
                base_url = excluded.base_url,
                model = excluded.model,
                options_json = excluded.options_json,
                has_secret = excluded.has_secret,
                secret_hint = excluded.secret_hint,
                enabled = excluded.enabled,
                updated_at = excluded.updated_at",
            params![
                provider.id,
                provider.kind,
                provider.provider_type,
                provider.name,
                provider.base_url,
                provider.model,
                provider.options_json,
                provider.has_secret,
                provider.secret_hint,
                provider.enabled,
                provider.created_at,
                provider.updated_at,
            ],
        )?;
        Ok(())
    }

    fn get(&self, id: &str) -> Result<Option<Provider>> {
        let mut stmt = self.conn.prepare("SELECT * FROM providers WHERE id = ?1")?;
        let provider = stmt.query_row(params![id], Provider::from_row).optional()?;
        Ok(provider)
    }

    fn list(&self, kind: Option<&str>) -> Result<Vec<Provider>> {
        let mut stmt = self.conn.prepare(
            "SELECT * FROM providers
             WHERE (?1 IS NULL OR kind = ?1)
             ORDER BY created_at ASC, name ASC",
        )?;
        let rows = stmt.query_map(params![kind], Provider::from_row)?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    fn set_enabled(&self, id: &str, enabled: bool) -> Result<()> {
        self.conn.execute(
            "UPDATE providers SET enabled = ?1, updated_at = ?2 WHERE id = ?3",
            params![enabled, Utc::now().timestamp(), id],
        )?;
        Ok(())
    }

    fn delete(&self, id: &str) -> Result<()> {
        self.conn
            .execute("DELETE FROM providers WHERE id = ?1", params![id])?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::run_migrations;

    fn setup() -> Connection {
        let mut conn = Connection::open_in_memory().expect("open in-memory db");
        run_migrations(&mut conn).expect("run migrations");
        conn
    }

    #[test]
    fn save_get_list_roundtrip() {
        let conn = setup();
        let repo = SqliteProviderRepository::new(&conn);

        let mut stt = Provider::new(
            KIND_STT,
            "local_whisper",
            "Whisper local",
            "ggml-large-v3-turbo-q5_0",
        );
        stt.has_secret = false;
        let mut llm = Provider::new(KIND_LLM, "openai_compat", "Meu LLM", "gpt-4o-mini");
        llm.base_url = Some("https://api.example.com".to_string());
        llm.has_secret = true;
        llm.secret_hint = Some("1234".to_string());

        repo.save(&stt).expect("save stt");
        repo.save(&llm).expect("save llm");

        let fetched = repo.get(&stt.id).expect("get").expect("exists");
        assert_eq!(fetched.name, "Whisper local");
        assert!(!fetched.has_secret);

        assert_eq!(repo.list(None).expect("list").len(), 2);
        assert_eq!(repo.list(Some(KIND_LLM)).expect("list llm").len(), 1);
        assert_eq!(
            repo.list(Some(KIND_STT)).expect("list stt")[0].provider_type,
            "local_whisper"
        );
    }

    #[test]
    fn save_conflict_updates_and_toggles_enabled() {
        let conn = setup();
        let repo = SqliteProviderRepository::new(&conn);

        let mut p = Provider::new(KIND_STT, "openai", "OpenAI", "whisper-1");
        repo.save(&p).expect("save");
        p.name = "OpenAI (renamed)".to_string();
        p.updated_at += 10;
        repo.save(&p).expect("re-save");

        let fetched = repo.get(&p.id).expect("get").expect("exists");
        assert_eq!(fetched.name, "OpenAI (renamed)");

        repo.set_enabled(&p.id, false).expect("disable");
        assert!(!repo.get(&p.id).expect("get").expect("exists").enabled);
    }

    #[test]
    fn delete_removes_provider() {
        let conn = setup();
        let repo = SqliteProviderRepository::new(&conn);
        let p = Provider::new(KIND_STT, "groq", "Groq", "whisper-large-v3");
        repo.save(&p).expect("save");
        repo.delete(&p.id).expect("delete");
        assert!(repo.get(&p.id).expect("get").is_none());
    }

    #[test]
    fn invalid_kind_is_rejected_by_check() {
        let conn = setup();
        let repo = SqliteProviderRepository::new(&conn);
        let p = Provider::new("bogus", "openai", "X", "m");
        assert!(repo.save(&p).is_err());
    }
}
