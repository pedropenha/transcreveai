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
    // --- Live transcription bookkeeping (11, T-065) ---------------------------
    // `meeting_segments.excluded` flags mic speech inside a dictation interval
    // (FR-009-10/AC-009-03: excluded from the transcript, represented by the
    // `dictation_marker` covering the same span).
    //
    // `meeting_blocks` is the persisted pending queue: the session worker
    // inserts one row per sealed block (`transcribed = 0`) and the live
    // transcriber flips `transcribed` once the block is fully processed.
    // Rows left at 0 after the meeting ends are exactly the work T-067's
    // post-processing still owes — the WAV path is derived from
    // `meetings.audio_dir` + the block naming convention
    // (`<track>-<index:04>.wav`), so it is not stored here.
    M::up(
        "ALTER TABLE meeting_segments ADD COLUMN excluded INTEGER NOT NULL DEFAULT 0;
        CREATE TABLE meeting_blocks (
            meeting_id    TEXT NOT NULL REFERENCES meetings(id) ON DELETE CASCADE,
            track         TEXT NOT NULL CHECK (track IN ('mic','system')),
            block_index   INTEGER NOT NULL,
            start_ms      INTEGER NOT NULL,
            end_ms        INTEGER NOT NULL,
            transcribed   INTEGER NOT NULL DEFAULT 0,
            attempts      INTEGER NOT NULL DEFAULT 0,
            PRIMARY KEY (meeting_id, track, block_index)
        );",
    ),
];
