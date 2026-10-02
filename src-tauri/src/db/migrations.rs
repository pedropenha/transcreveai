//! Versioned SQLite migrations for `transcreve-ai.db`.
//!
//! Each `M::up` is applied once, in order, inside its own transaction;
//! `rusqlite_migration` tracks progress in SQLite's `user_version` pragma.
//!
//! Migrations 1–4 are the Handy-inherited `transcription_history` history and
//! must never change (they already ran on user databases). Migrations 5+ align
//! the schema with `specs/architecture/data-model.md` §2: they create the new
//! domain tables and rebuild `transcription_history` into `dictations`,
//! preserving every existing row.
//!
//! Deviations from the spec schema (allowed by data-model.md: "nomes de colunas
//! podem se ajustar ao que o fork já tiver"):
//! - `dictations.id` stays `INTEGER AUTOINCREMENT` (the fork's `rowid` is used
//!   by IPC events and the frontend), not a TEXT uuid.
//! - `dictations` keeps three fork-only columns: `title` (UI label),
//!   `post_processed_text` (the raw LLM output when post-processing ran —
//!   `final_text` holds the effective delivered text) and
//!   `post_process_requested`.

use rusqlite_migration::M;

pub(crate) static MIGRATIONS: &[M] = &[
    // --- Handy-inherited history schema (1–4, immutable) ---------------------
    M::up(
        "CREATE TABLE IF NOT EXISTS transcription_history (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            file_name TEXT NOT NULL,
            timestamp INTEGER NOT NULL,
            saved BOOLEAN NOT NULL DEFAULT 0,
            title TEXT NOT NULL,
            transcription_text TEXT NOT NULL
        );",
    ),
    M::up("ALTER TABLE transcription_history ADD COLUMN post_processed_text TEXT;"),
    M::up("ALTER TABLE transcription_history ADD COLUMN post_process_prompt TEXT;"),
    M::up(
        "ALTER TABLE transcription_history ADD COLUMN post_process_requested BOOLEAN NOT NULL DEFAULT 0;",
    ),
    // --- Providers and local models (5) --------------------------------------
    M::up(
        "CREATE TABLE providers (
            id            TEXT PRIMARY KEY,
            kind          TEXT NOT NULL CHECK (kind IN ('stt','llm')),
            type          TEXT NOT NULL,
            name          TEXT NOT NULL,
            base_url      TEXT,
            model         TEXT NOT NULL,
            options_json  TEXT NOT NULL DEFAULT '{}',
            has_secret    INTEGER NOT NULL DEFAULT 0,
            secret_hint   TEXT,
            enabled       INTEGER NOT NULL DEFAULT 1,
            created_at    INTEGER NOT NULL,
            updated_at    INTEGER NOT NULL
        );
        CREATE TABLE local_models (
            id            TEXT PRIMARY KEY,
            engine        TEXT NOT NULL,
            file_path     TEXT NOT NULL,
            size_bytes    INTEGER NOT NULL,
            sha256        TEXT NOT NULL,
            status        TEXT NOT NULL CHECK (status IN ('downloading','ready','error')),
            downloaded_at INTEGER
        );",
    ),
    // --- Dictionary, snippets, app profiles, transforms (6) ------------------
    M::up(
        "CREATE TABLE dictionary_entries (
            id            TEXT PRIMARY KEY,
            term          TEXT NOT NULL,
            kind          TEXT NOT NULL CHECK (kind IN ('vocab','replacement')),
            match_text    TEXT,
            case_sensitive INTEGER NOT NULL DEFAULT 0,
            source        TEXT NOT NULL DEFAULT 'manual' CHECK (source IN ('manual','auto')),
            created_at    INTEGER NOT NULL
        );
        CREATE TABLE snippets (
            id            TEXT PRIMARY KEY,
            trigger       TEXT NOT NULL UNIQUE,
            expansion     TEXT NOT NULL,
            match_mode    TEXT NOT NULL DEFAULT 'inline' CHECK (match_mode IN ('whole','inline')),
            enabled       INTEGER NOT NULL DEFAULT 1,
            use_count     INTEGER NOT NULL DEFAULT 0
        );
        CREATE TABLE app_profiles (
            id            TEXT PRIMARY KEY,
            name          TEXT NOT NULL,
            match_exe     TEXT,
            match_title   TEXT,
            category      TEXT NOT NULL CHECK (category IN ('email','work_chat','personal_chat','code','terminal','docs','other')),
            style         TEXT NOT NULL DEFAULT 'default' CHECK (style IN ('default','formal','casual','very_casual','technical')),
            cleanup_level TEXT CHECK (cleanup_level IN ('none','light','medium','high')),
            custom_prompt TEXT,
            insertion_method TEXT NOT NULL DEFAULT 'auto' CHECK (insertion_method IN ('auto','paste','paste_shift_insert','type','clipboard_only')),
            newline_mode  TEXT NOT NULL DEFAULT 'raw' CHECK (newline_mode IN ('raw','shift_enter')),
            priority      INTEGER NOT NULL DEFAULT 0,
            builtin       INTEGER NOT NULL DEFAULT 0
        );
        CREATE TABLE transforms (
            id      TEXT PRIMARY KEY,
            name    TEXT NOT NULL,
            prompt  TEXT NOT NULL,
            hotkey  TEXT
        );",
    ),
    // --- Meetings domain and notes (7) ----------------------------------------
    // summary_templates must exist before meetings (foreign key).
    M::up(
        "CREATE TABLE summary_templates (
            id            TEXT PRIMARY KEY,
            name          TEXT NOT NULL,
            prompt        TEXT NOT NULL,
            is_default    INTEGER NOT NULL DEFAULT 0,
            builtin       INTEGER NOT NULL DEFAULT 0
        );
        CREATE TABLE meetings (
            id            TEXT PRIMARY KEY,
            title         TEXT NOT NULL,
            app_exe       TEXT,
            app_label     TEXT,
            detection     TEXT NOT NULL CHECK (detection IN ('auto_prompt','auto_start','manual','in_person')),
            status        TEXT NOT NULL CHECK (status IN ('recording','paused','processing','ready','error','recovered')),
            started_at    INTEGER NOT NULL,
            ended_at      INTEGER,
            capture_system_audio INTEGER NOT NULL DEFAULT 1,
            stt_provider_id TEXT REFERENCES providers(id) ON DELETE SET NULL,
            llm_provider_id TEXT REFERENCES providers(id) ON DELETE SET NULL,
            template_id   TEXT REFERENCES summary_templates(id) ON DELETE SET NULL,
            summary_md    TEXT,
            audio_dir     TEXT,
            language      TEXT,
            error_code    TEXT
        );
        CREATE TABLE meeting_segments (
            id            TEXT PRIMARY KEY,
            meeting_id    TEXT NOT NULL REFERENCES meetings(id) ON DELETE CASCADE,
            track         TEXT NOT NULL CHECK (track IN ('mic','system')),
            speaker       TEXT,
            start_ms      INTEGER NOT NULL,
            end_ms        INTEGER NOT NULL,
            text          TEXT NOT NULL,
            kind          TEXT NOT NULL DEFAULT 'speech' CHECK (kind IN ('speech','dictation_marker','gap_marker')),
            is_final      INTEGER NOT NULL DEFAULT 1
        );
        CREATE INDEX idx_segments_meeting ON meeting_segments(meeting_id, start_ms);
        CREATE TABLE meeting_app_rules (
            id            TEXT PRIMARY KEY,
            exe           TEXT NOT NULL,
            title_pattern TEXT,
            label         TEXT NOT NULL,
            action        TEXT NOT NULL CHECK (action IN ('ask','auto_start','ignore')),
            builtin       INTEGER NOT NULL DEFAULT 0
        );
        CREATE TABLE notes (
            id            TEXT PRIMARY KEY,
            title         TEXT NOT NULL DEFAULT '',
            body_md       TEXT NOT NULL DEFAULT '',
            source        TEXT NOT NULL CHECK (source IN ('scratchpad','voice','meeting')),
            meeting_id    TEXT REFERENCES meetings(id) ON DELETE CASCADE,
            pinned        INTEGER NOT NULL DEFAULT 0,
            created_at    INTEGER NOT NULL,
            updated_at    INTEGER NOT NULL,
            exported_path TEXT
        );",
    ),
    // --- Rebuild transcription_history into dictations (8) --------------------
    // Non-destructive: every legacy column is mapped onto the new schema.
    // `final_text` (NOT NULL per spec) is backfilled with the post-processed
    // text when it exists, falling back to the raw transcription.
    M::up(
        "CREATE TABLE dictations (
            id            INTEGER PRIMARY KEY AUTOINCREMENT,
            created_at    INTEGER NOT NULL,
            mode          TEXT NOT NULL DEFAULT 'dictation' CHECK (mode IN ('dictation','command','note')),
            duration_ms   INTEGER NOT NULL DEFAULT 0,
            app_exe       TEXT,
            app_name      TEXT,
            stt_provider_id TEXT REFERENCES providers(id) ON DELETE SET NULL,
            llm_provider_id TEXT REFERENCES providers(id) ON DELETE SET NULL,
            language      TEXT,
            raw_text      TEXT NOT NULL,
            final_text    TEXT NOT NULL,
            instruction   TEXT,
            status        TEXT NOT NULL DEFAULT 'inserted' CHECK (status IN ('inserted','copied','failed','cancelled','saved_note')),
            error_code    TEXT,
            latency_json  TEXT NOT NULL DEFAULT '{}',
            audio_path    TEXT,
            word_count    INTEGER NOT NULL DEFAULT 0,
            flagged       INTEGER NOT NULL DEFAULT 0,
            post_processed_text TEXT,
            title         TEXT NOT NULL DEFAULT '',
            post_process_requested INTEGER NOT NULL DEFAULT 0
        );
        INSERT INTO dictations (
            id, created_at, raw_text, final_text, instruction,
            audio_path, flagged, title, post_processed_text,
            post_process_requested, status, word_count
        )
        SELECT
            id, timestamp, transcription_text,
            COALESCE(post_processed_text, transcription_text),
            post_process_prompt, file_name, saved, title,
            post_processed_text, post_process_requested, 'inserted',
            CASE WHEN COALESCE(post_processed_text, transcription_text) = ''
                 THEN 0
                 ELSE LENGTH(COALESCE(post_processed_text, transcription_text))
                    - LENGTH(REPLACE(COALESCE(post_processed_text, transcription_text), ' ', ''))
                    + 1
            END
        FROM transcription_history;
        DROP TABLE transcription_history;
        CREATE INDEX idx_dictations_created ON dictations(created_at DESC);",
    ),
    // --- FTS5 search indexes + sync triggers (9) ------------------------------
    M::up(
        "CREATE VIRTUAL TABLE dictations_fts USING fts5(
            final_text, raw_text,
            content='dictations', content_rowid='rowid'
        );
        CREATE VIRTUAL TABLE notes_fts USING fts5(
            title, body_md,
            content='notes', content_rowid='rowid'
        );
        CREATE VIRTUAL TABLE meeting_fts USING fts5(
            text,
            content='meeting_segments', content_rowid='rowid'
        );
        CREATE TRIGGER dictations_fts_ai AFTER INSERT ON dictations BEGIN
            INSERT INTO dictations_fts(rowid, final_text, raw_text)
            VALUES (new.rowid, new.final_text, new.raw_text);
        END;
        CREATE TRIGGER dictations_fts_ad AFTER DELETE ON dictations BEGIN
            INSERT INTO dictations_fts(dictations_fts, rowid, final_text, raw_text)
            VALUES ('delete', old.rowid, old.final_text, old.raw_text);
        END;
        CREATE TRIGGER dictations_fts_au AFTER UPDATE ON dictations BEGIN
            INSERT INTO dictations_fts(dictations_fts, rowid, final_text, raw_text)
            VALUES ('delete', old.rowid, old.final_text, old.raw_text);
            INSERT INTO dictations_fts(rowid, final_text, raw_text)
            VALUES (new.rowid, new.final_text, new.raw_text);
        END;
        CREATE TRIGGER notes_fts_ai AFTER INSERT ON notes BEGIN
            INSERT INTO notes_fts(rowid, title, body_md)
            VALUES (new.rowid, new.title, new.body_md);
        END;
        CREATE TRIGGER notes_fts_ad AFTER DELETE ON notes BEGIN
            INSERT INTO notes_fts(notes_fts, rowid, title, body_md)
            VALUES ('delete', old.rowid, old.title, old.body_md);
        END;
        CREATE TRIGGER notes_fts_au AFTER UPDATE ON notes BEGIN
            INSERT INTO notes_fts(notes_fts, rowid, title, body_md)
            VALUES ('delete', old.rowid, old.title, old.body_md);
            INSERT INTO notes_fts(rowid, title, body_md)
            VALUES (new.rowid, new.title, new.body_md);
        END;
        CREATE TRIGGER meeting_fts_ai AFTER INSERT ON meeting_segments BEGIN
            INSERT INTO meeting_fts(rowid, text)
            VALUES (new.rowid, new.text);
        END;
        CREATE TRIGGER meeting_fts_ad AFTER DELETE ON meeting_segments BEGIN
            INSERT INTO meeting_fts(meeting_fts, rowid, text)
            VALUES ('delete', old.rowid, old.text);
        END;
        CREATE TRIGGER meeting_fts_au AFTER UPDATE ON meeting_segments BEGIN
            INSERT INTO meeting_fts(meeting_fts, rowid, text)
            VALUES ('delete', old.rowid, old.text);
            INSERT INTO meeting_fts(rowid, text)
            VALUES (new.rowid, new.text);
        END;
        INSERT INTO dictations_fts(dictations_fts) VALUES ('rebuild');
        INSERT INTO notes_fts(notes_fts) VALUES ('rebuild');
        INSERT INTO meeting_fts(meeting_fts) VALUES ('rebuild');",
    ),
    // --- Builtin meeting-app detection rules (10, T-060) ----------------------
    // Closed v1 set per spec F008/ADR-0002: Zoom, Teams, Meet (browser) and
    // Webex. `action='ask'` is the default posture; the user flips to
    // 'auto_start'/'ignore' from the toast (T-062) or rules settings.
    //
    // title_pattern semantics in `meeting::classifier`: when set, a window of
    // the same exe must match the regex (mandatory for browsers — an exe match
    // alone is never enough). When NULL, the exe match suffices, which is why
    // Zoom/Teams/Webex ship without a pattern.
    //
    // `MSTeams` is not an exe name: new Teams is packaged, so its ConsentStore
    // subkey is `MSTeams_<publisher>!MSTeams` and the classifier's `exe_name`
    // is the `!` tail.
    //
    // Ids are deterministic so later migrations can UPDATE them (regex fixes)
    // and INSERT OR IGNORE keeps re-runs and edited copies conflict-free.
    M::up(
        "INSERT OR IGNORE INTO meeting_app_rules (id, exe, title_pattern, label, action, builtin) VALUES
            ('builtin-zoom',         'Zoom.exe',           NULL, 'Zoom',            'ask', 1),
            ('builtin-teams-new',    'ms-teams.exe',       NULL, 'Microsoft Teams', 'ask', 1),
            ('builtin-teams-classic','Teams.exe',          NULL, 'Microsoft Teams', 'ask', 1),
            ('builtin-teams-packaged','MSTeams',           NULL, 'Microsoft Teams', 'ask', 1),
            ('builtin-meet-chrome',  'chrome.exe',  '^Meet -|meet\\.google\\.com', 'Google Meet', 'ask', 1),
            ('builtin-meet-edge',    'msedge.exe',  '^Meet -|meet\\.google\\.com', 'Google Meet', 'ask', 1),
            ('builtin-meet-firefox', 'firefox.exe', '^Meet -|meet\\.google\\.com', 'Google Meet', 'ask', 1),
            ('builtin-meet-brave',   'brave.exe',   '^Meet -|meet\\.google\\.com', 'Google Meet', 'ask', 1),
            ('builtin-meet-arc',     'arc.exe',     '^Meet -|meet\\.google\\.com', 'Google Meet', 'ask', 1),
            ('builtin-webex-host',   'CiscoCollabHost.exe', NULL, 'Webex',          'ask', 1),
            ('builtin-webex-mta',    'webexmta.exe',        NULL, 'Webex',          'ask', 1);",
    ),
    // --- Live transcription bookkeeping + summary state (11, T-065/T-067) ----
    // `meeting_segments.excluded` flags mic speech inside a dictation interval
    // (FR-009-10/AC-009-03: excluded from the transcript, represented by the
    // `dictation_marker` covering the same span).
    //
    // `meeting_blocks` is the persisted per-block record: the session worker
    // inserts one row per sealed block (`transcribed = 0`) — durable clock
    // placement for post-processing AND the pending queue the live
    // transcriber drains. Rows left at 0 after the meeting ends are exactly
    // the work post-processing still owes. The WAV path is derived, not
    // stored: `meetings.audio_dir` + `<track>-<index:04>.wav`.
    //
    // `summary_status` tracks FR-009-16 step (4) independently of the meeting
    // status: FR-009-21 needs `ready` meetings whose summary is `disabled`
    // (no BYOK key) to be distinguishable from `ready`+`error` (retryable).
    // ADD COLUMN supports CHECK but forbids UNIQUE/PK; the default keeps
    // existing rows at 'pending'.
    //
    // The pt-BR template is the spec's FR-009-17 default (fixed id so later
    // migrations can UPDATE its text); `is_default` is what the pipeline reads
    // when a meeting row carries no `template_id` override.
    M::up(
        "ALTER TABLE meeting_segments ADD COLUMN excluded INTEGER NOT NULL DEFAULT 0;
        ALTER TABLE meetings ADD COLUMN summary_status TEXT NOT NULL DEFAULT 'pending'
            CHECK (summary_status IN ('pending','ready','disabled','error'));
        CREATE TABLE meeting_blocks (
            meeting_id    TEXT NOT NULL REFERENCES meetings(id) ON DELETE CASCADE,
            track         TEXT NOT NULL CHECK (track IN ('mic','system')),
            block_index   INTEGER NOT NULL,
            start_ms      INTEGER NOT NULL,
            end_ms        INTEGER NOT NULL,
            transcribed   INTEGER NOT NULL DEFAULT 0,
            attempts      INTEGER NOT NULL DEFAULT 0,
            PRIMARY KEY (meeting_id, track, block_index)
        );
        INSERT OR IGNORE INTO summary_templates (id, name, prompt, is_default, builtin) VALUES (
            'builtin-default',
            'Padrão',
            'Você é o assistente de atas do Transcreve.ai. Resuma a reunião seguindo exatamente esta estrutura Markdown:

## Resumo

(3 a 5 tópicos)

## Decisões

## Próximos passos

- [ ] Tarefa — Responsável (ou Todos) — Prazo (se mencionado)

## Pontos em aberto

## Tópicos discutidos

- Tópico (mm:ss)

Regras: nunca invente responsáveis nem prazos; tarefas do grupo ficam como Todos; trate Minhas notas do usuário como contexto prioritário; escreva no idioma predominante da reunião; cite horários (mm:ss) quando relevante.',
            1,
            1);",
    ),
    // --- FTS over meeting title + summary (12, T-068) -------------------------
    // FR-009-25 searches title, notes, summary and transcript. Migration 9
    // already indexes `meeting_segments.text` (meeting_fts) and `notes`
    // (notes_fts); `meeting_search` unions the three sources, so this table
    // only needs to cover what was missing: `meetings.title` and
    // `meetings.summary_md`. Same external-content + trigger shape as the
    // existing FTS tables.
    M::up(
        "CREATE VIRTUAL TABLE meetings_fts USING fts5(
            title, summary_md,
            content='meetings', content_rowid='rowid'
        );
        CREATE TRIGGER meetings_fts_ai AFTER INSERT ON meetings BEGIN
            INSERT INTO meetings_fts(rowid, title, summary_md)
            VALUES (new.rowid, new.title, new.summary_md);
        END;
        CREATE TRIGGER meetings_fts_ad AFTER DELETE ON meetings BEGIN
            INSERT INTO meetings_fts(meetings_fts, rowid, title, summary_md)
            VALUES ('delete', old.rowid, old.title, old.summary_md);
        END;
        CREATE TRIGGER meetings_fts_au AFTER UPDATE ON meetings BEGIN
            INSERT INTO meetings_fts(meetings_fts, rowid, title, summary_md)
            VALUES ('delete', old.rowid, old.title, old.summary_md);
            INSERT INTO meetings_fts(rowid, title, summary_md)
            VALUES (new.rowid, new.title, new.summary_md);
        END;
        INSERT INTO meetings_fts(meetings_fts) VALUES ('rebuild');",
    ),

    // --- meeting_fts honors `excluded` (13, privacy fix) ----------------------
    // Migration 9's meeting_fts triggers index every segment unconditionally,
    // so dictated mic rows (`excluded = 1`, FR-009-10/AC-009-03) stayed
    // searchable. The triggers are recreated with exclusion guards; the update
    // trigger uses INSERT...SELECT so an `excluded` flip in either direction
    // keeps the index in sync. A manual 'delete-all' + filtered repopulate is
    // used instead of 'rebuild' because FTS5's rebuild command has no WHERE
    // clause and would re-index the excluded rows.
    M::up(
        "DROP TRIGGER IF EXISTS meeting_fts_ai;
        DROP TRIGGER IF EXISTS meeting_fts_ad;
        DROP TRIGGER IF EXISTS meeting_fts_au;
        CREATE TRIGGER meeting_fts_ai AFTER INSERT ON meeting_segments
            WHEN new.excluded = 0
        BEGIN
            INSERT INTO meeting_fts(rowid, text)
            VALUES (new.rowid, new.text);
        END;
        CREATE TRIGGER meeting_fts_ad AFTER DELETE ON meeting_segments
            WHEN old.excluded = 0
        BEGIN
            INSERT INTO meeting_fts(meeting_fts, rowid, text)
            VALUES ('delete', old.rowid, old.text);
        END;
        CREATE TRIGGER meeting_fts_au AFTER UPDATE ON meeting_segments BEGIN
            INSERT INTO meeting_fts(meeting_fts, rowid, text)
            SELECT 'delete', old.rowid, old.text WHERE old.excluded = 0;
            INSERT INTO meeting_fts(rowid, text)
            SELECT new.rowid, new.text WHERE new.excluded = 0;
        END;
        INSERT INTO meeting_fts(meeting_fts) VALUES ('delete-all');
        INSERT INTO meeting_fts(rowid, text)
        SELECT rowid, text FROM meeting_segments WHERE excluded = 0;",
    ),

    // --- dictations.status gains 'routed' (14, F012 voice assistant) --------
    // FR-012-12: a dictation claimed by the assistant panel is marked
    // 'routed' by `actions.rs` — the text went to the panel input instead of
    // being pasted. SQLite cannot alter a CHECK constraint, so the table is
    // rebuilt the canonical way: new table → copy rows verbatim (ids too, so
    // `sqlite_sequence` follows the rename) → drop old → rename.
    // `dictations_fts` is external-content keyed by rowid; since rowids are
    // preserved the shadow table survives untouched — only its sync triggers
    // (dropped with the old table) are recreated.
    M::up(
        "CREATE TABLE dictations_new (
            id            INTEGER PRIMARY KEY AUTOINCREMENT,
            created_at    INTEGER NOT NULL,
            mode          TEXT NOT NULL DEFAULT 'dictation' CHECK (mode IN ('dictation','command','note')),
            duration_ms   INTEGER NOT NULL DEFAULT 0,
            app_exe       TEXT,
            app_name      TEXT,
            stt_provider_id TEXT REFERENCES providers(id) ON DELETE SET NULL,
            llm_provider_id TEXT REFERENCES providers(id) ON DELETE SET NULL,
            language      TEXT,
            raw_text      TEXT NOT NULL,
            final_text    TEXT NOT NULL,
            instruction   TEXT,
            status        TEXT NOT NULL DEFAULT 'inserted' CHECK (status IN ('inserted','copied','failed','cancelled','saved_note','routed')),
            error_code    TEXT,
            latency_json  TEXT NOT NULL DEFAULT '{}',
            audio_path    TEXT,
            word_count    INTEGER NOT NULL DEFAULT 0,
            flagged       INTEGER NOT NULL DEFAULT 0,
            post_processed_text TEXT,
            title         TEXT NOT NULL DEFAULT '',
            post_process_requested INTEGER NOT NULL DEFAULT 0
        );
        INSERT INTO dictations_new (
            id, created_at, mode, duration_ms, app_exe, app_name,
            stt_provider_id, llm_provider_id, language,
            raw_text, final_text, instruction, status, error_code,
            latency_json, audio_path, word_count, flagged,
            post_processed_text, title, post_process_requested
        )
        SELECT
            id, created_at, mode, duration_ms, app_exe, app_name,
            stt_provider_id, llm_provider_id, language,
            raw_text, final_text, instruction, status, error_code,
            latency_json, audio_path, word_count, flagged,
            post_processed_text, title, post_process_requested
        FROM dictations;
        DROP TABLE dictations;
        ALTER TABLE dictations_new RENAME TO dictations;
        CREATE INDEX idx_dictations_created ON dictations(created_at DESC);
        CREATE TRIGGER dictations_fts_ai AFTER INSERT ON dictations BEGIN
            INSERT INTO dictations_fts(rowid, final_text, raw_text)
            VALUES (new.rowid, new.final_text, new.raw_text);
        END;
        CREATE TRIGGER dictations_fts_ad AFTER DELETE ON dictations BEGIN
            INSERT INTO dictations_fts(dictations_fts, rowid, final_text, raw_text)
            VALUES ('delete', old.rowid, old.final_text, old.raw_text);
        END;
        CREATE TRIGGER dictations_fts_au AFTER UPDATE ON dictations BEGIN
            INSERT INTO dictations_fts(dictations_fts, rowid, final_text, raw_text)
            VALUES ('delete', old.rowid, old.final_text, old.raw_text);
            INSERT INTO dictations_fts(rowid, final_text, raw_text)
            VALUES (new.rowid, new.final_text, new.raw_text);
        END;",
    ),
    // --- meetings.app_exe_path (15, Notetaker list: source app icon) ----------
    // The Notetaker list shows each meeting's source-app logo extracted from
    // the detected executable. `app_exe` (file name) + `app_label` (friendly
    // name) already form the `source_app` pair; what a restart loses is the
    // *path* the icon is extracted from, so it is persisted here. Nullable and
    // additive: existing rows keep every column and simply have no path (the
    // UI falls back to a monogram). Written only after `is_safe_exe_path`.
    M::up("ALTER TABLE meetings ADD COLUMN app_exe_path TEXT;"),
];
