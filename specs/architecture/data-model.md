# Modelo de Dados

## 1. Arquivos em disco

```
%APPDATA%\br.com.creator4all.transcreve.ai\   # pasta do identifier (app_data_dir do Tauri)
├── transcreve-ai.db            # SQLite (WAL)
├── settings.json               # configurações (sem segredos), schema versionado
├── models\
│   ├── whisper\ggml-large-v3-turbo-q5_0.bin
│   └── parakeet\parakeet-tdt-0.6b-v3-int8\...
├── audio\
│   ├── dictations\<uuid>.wav   # retenção curta (retry)
│   └── meetings\<uuid>\
│       ├── mic-0001.wav …      # blocos de 60 s (à prova de crash)
│       └── system-0001.wav …
└── logs\sussurro-YYYY-MM-DD.log
```

Chaves de API: **fora daqui**, no cofre do SO (entrada `Transcreve.ai/provider/<provider_id>`).

## 2. Esquema SQLite

```sql
-- Provedores de STT/LLM configurados
CREATE TABLE providers (
  id            TEXT PRIMARY KEY,               -- uuid
  kind          TEXT NOT NULL CHECK (kind IN ('stt','llm')),
  type          TEXT NOT NULL,                  -- local_whisper | local_parakeet | openai | groq | deepgram | openai_compat | anthropic | ollama
  name          TEXT NOT NULL,                  -- rótulo do usuário
  base_url      TEXT,                           -- para openai_compat/ollama
  model         TEXT NOT NULL,                  -- ex.: gpt-4o-mini-transcribe, whisper-large-v3-turbo, ggml-large-v3-turbo-q5_0
  options_json  TEXT NOT NULL DEFAULT '{}',     -- temperatura, idioma, timeouts…
  has_secret    INTEGER NOT NULL DEFAULT 0,
  secret_hint   TEXT,                           -- últimos 4 caracteres, só para exibição
  enabled       INTEGER NOT NULL DEFAULT 1,
  created_at    INTEGER NOT NULL,
  updated_at    INTEGER NOT NULL
);

-- Modelos locais baixados
CREATE TABLE local_models (
  id            TEXT PRIMARY KEY,               -- id do catálogo, ex.: whisper-large-v3-turbo-q5_0
  engine        TEXT NOT NULL,                  -- whisper | parakeet
  file_path     TEXT NOT NULL,
  size_bytes    INTEGER NOT NULL,
  sha256        TEXT NOT NULL,
  status        TEXT NOT NULL CHECK (status IN ('downloading','ready','error')),
  downloaded_at INTEGER
);

-- Histórico de ditados
CREATE TABLE dictations (
  id            TEXT PRIMARY KEY,
  created_at    INTEGER NOT NULL,
  mode          TEXT NOT NULL CHECK (mode IN ('dictation','command','note')),
  duration_ms   INTEGER NOT NULL,               -- duração do áudio
  app_exe       TEXT,                           -- app em foco no INÍCIO da sessão, ex.: Claude.exe (F010 FR-010-29)
  app_name      TEXT,                           -- rótulo amigável, ex.: Claude, Chrome, Visual Studio Code
  app_exe_path  TEXT,                           -- caminho do .exe de origem, só para extrair o ícone (migração 16); sanitizado por `is_safe_exe_path`; nunca sai por IPC
  stt_provider_id TEXT REFERENCES providers(id) ON DELETE SET NULL,
  llm_provider_id TEXT REFERENCES providers(id) ON DELETE SET NULL,
  language      TEXT,
  raw_text      TEXT NOT NULL,
  final_text    TEXT NOT NULL,
  instruction   TEXT,                           -- Command Mode: instrução falada
  status        TEXT NOT NULL CHECK (status IN ('inserted','copied','failed','cancelled','saved_note')),
  error_code    TEXT,
  latency_json  TEXT NOT NULL DEFAULT '{}',     -- {stt_ms, llm_ms, insert_ms, total_ms}
  audio_path    TEXT,                           -- NULL após expirar a retenção
  word_count    INTEGER NOT NULL DEFAULT 0,
  flagged       INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX idx_dictations_created ON dictations(created_at DESC);
CREATE VIRTUAL TABLE dictations_fts USING fts5(final_text, raw_text, content='dictations', content_rowid='rowid');

-- Dicionário
CREATE TABLE dictionary_entries (
  id            TEXT PRIMARY KEY,
  term          TEXT NOT NULL,                  -- forma correta (ex.: "Kubernetes", "Transcreve.ai")
  kind          TEXT NOT NULL CHECK (kind IN ('vocab','replacement')),
  match_text    TEXT,                           -- para 'replacement': o que o ASR costuma errar (ex.: "cuber netes")
  case_sensitive INTEGER NOT NULL DEFAULT 0,
  source        TEXT NOT NULL DEFAULT 'manual' CHECK (source IN ('manual','auto')),
  created_at    INTEGER NOT NULL
);

-- Snippets
CREATE TABLE snippets (
  id            TEXT PRIMARY KEY,
  trigger       TEXT NOT NULL UNIQUE,           -- frase falada (normalizada na comparação)
  expansion     TEXT NOT NULL,                  -- suporta {date} {time} {clipboard}
  match_mode    TEXT NOT NULL DEFAULT 'inline' CHECK (match_mode IN ('whole','inline')),
  enabled       INTEGER NOT NULL DEFAULT 1,
  use_count     INTEGER NOT NULL DEFAULT 0
);

-- Perfis por aplicativo (estilos)
CREATE TABLE app_profiles (
  id            TEXT PRIMARY KEY,
  name          TEXT NOT NULL,
  match_exe     TEXT,                           -- ex.: slack.exe (case-insensitive)
  match_title   TEXT,                           -- regex opcional (ex.: navegador com "Gmail")
  category      TEXT NOT NULL CHECK (category IN ('email','work_chat','personal_chat','code','terminal','docs','other')),
  style         TEXT NOT NULL DEFAULT 'default' CHECK (style IN ('default','formal','casual','very_casual','technical')),
  cleanup_level TEXT CHECK (cleanup_level IN ('none','light','medium','high')), -- NULL = herda global
  custom_prompt TEXT,
  insertion_method TEXT NOT NULL DEFAULT 'auto' CHECK (insertion_method IN ('auto','paste','paste_shift_insert','type','clipboard_only')),
  newline_mode  TEXT NOT NULL DEFAULT 'raw' CHECK (newline_mode IN ('raw','shift_enter')),
  priority      INTEGER NOT NULL DEFAULT 0,
  builtin       INTEGER NOT NULL DEFAULT 0
);

-- Transforms (prompts do Command Mode salvos) — P2
CREATE TABLE transforms (
  id TEXT PRIMARY KEY, name TEXT NOT NULL, prompt TEXT NOT NULL, hotkey TEXT
);

-- Notas (scratchpad e notas de reunião)
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
);
CREATE VIRTUAL TABLE notes_fts USING fts5(title, body_md, content='notes', content_rowid='rowid');

-- Reuniões
CREATE TABLE meetings (
  id            TEXT PRIMARY KEY,
  title         TEXT NOT NULL,
  app_exe       TEXT,                           -- Zoom.exe, chrome.exe…  ┐ `source_app` = (app_exe, app_label);
  app_label     TEXT,                           -- "Google Meet"          ┘ manuais/presenciais: NULL
  app_exe_path  TEXT,                           -- caminho do .exe detectado, só para extrair o ícone (migração 15);
                                                -- NULL em reuniões anteriores/sem detecção; só gravado se passar
                                                -- is_safe_exe_path (X:\…\*.exe local); nunca sai por IPC
  detection     TEXT NOT NULL CHECK (detection IN ('auto_prompt','auto_start','manual','in_person')),
  status        TEXT NOT NULL CHECK (status IN ('recording','paused','processing','ready','error','recovered')),
  started_at    INTEGER NOT NULL,
  ended_at      INTEGER,
  capture_system_audio INTEGER NOT NULL DEFAULT 1,
  stt_provider_id TEXT REFERENCES providers(id) ON DELETE SET NULL,
  llm_provider_id TEXT REFERENCES providers(id) ON DELETE SET NULL,
  template_id   TEXT REFERENCES summary_templates(id) ON DELETE SET NULL,
  summary_md    TEXT,                           -- editável
  audio_dir     TEXT,                           -- NULL após expirar retenção
  language      TEXT,
  error_code    TEXT
);

CREATE TABLE meeting_segments (
  id            TEXT PRIMARY KEY,
  meeting_id    TEXT NOT NULL REFERENCES meetings(id) ON DELETE CASCADE,
  track         TEXT NOT NULL CHECK (track IN ('mic','system')),
  speaker       TEXT,                           -- "Você", "Outros", "Falante 1"…
  start_ms      INTEGER NOT NULL,               -- relativo a started_at
  end_ms        INTEGER NOT NULL,
  text          TEXT NOT NULL,
  kind          TEXT NOT NULL DEFAULT 'speech' CHECK (kind IN ('speech','dictation_marker','gap_marker')),
  is_final      INTEGER NOT NULL DEFAULT 1
);
CREATE INDEX idx_segments_meeting ON meeting_segments(meeting_id, start_ms);
CREATE VIRTUAL TABLE meeting_fts USING fts5(text, content='meeting_segments', content_rowid='rowid');

CREATE TABLE summary_templates (
  id            TEXT PRIMARY KEY,
  name          TEXT NOT NULL,
  prompt        TEXT NOT NULL,
  is_default    INTEGER NOT NULL DEFAULT 0,
  builtin       INTEGER NOT NULL DEFAULT 0
);

-- Regras de detecção de reunião por app
CREATE TABLE meeting_app_rules (
  id            TEXT PRIMARY KEY,
  exe           TEXT NOT NULL,
  title_pattern TEXT,                           -- regex
  label         TEXT NOT NULL,                  -- "Zoom", "Google Meet"
  action        TEXT NOT NULL CHECK (action IN ('ask','auto_start','ignore')),
  builtin       INTEGER NOT NULL DEFAULT 0
);
```

**`source_app` e status de lista (Notetaker).** O "app de origem" de uma reunião é o par `app_exe` + `app_label` (já existiam); a migração 15 só acrescenta `app_exe_path`, nullable e aditiva (reuniões antigas ficam com `NULL` e a UI cai no monograma). Nada disso é coluna nova de status: o chip da linha (`Transcrevendo` / `Sem resumo` / `Falhou` / `Resumo pronto`) é **derivado** de `status` × `summary_status`:

| `status`               | `summary_status`                   | `list_status`                          |
| ---------------------- | ---------------------------------- | -------------------------------------- |
| `recording` / `paused` | —                                  | `recording` / `paused` (bloco "Agora") |
| `processing`           | —                                  | `processing`                           |
| `error` / `recovered`  | —                                  | `failed`                               |
| `ready`                | `pending`                          | `processing` (resumo em andamento)     |
| `ready`                | `disabled` (sem chave, FR-009-21)  | `no_summary`                           |
| `ready`                | `error`                            | `failed`                               |
| `ready`                | `ready` com `summary_md` não vazio | `ready`                                |
| `ready`                | `ready` sem `summary_md`           | `no_summary`                           |

Triggers mantêm as tabelas FTS sincronizadas. Migrações versionadas, aplicadas na inicialização dentro de transação (`rusqlite_migration`, herdado do Handy — [ADR-0001](../../docs/adr/0001-fork-do-handy-como-base.md)).

O acesso a dados segue o **Repository Pattern com traits** de `rules/rust/patterns.md` (implementação SQLite + implementação em memória para testes) e somente **queries parametrizadas** (`rules/rust/security.md`). O esquema acima é o alvo; nomes de colunas podem se ajustar ao que o fork já tiver.

## 3. `settings.json` (exemplo)

T-093 acrescenta ao `AppSettings` persistido a chave opcional `translation_model_id: string | null` (padrão `null`, compatível com stores anteriores). A chave seleciona um modelo local só para `transcribe_translate`; `null` herda o modelo do ditado se compatível. Não modifica `selected_model`, `dictation_provider_id`, `meeting_provider_id`, `fallback_provider_id` nem `translate_to_english`. As opções de execução da tradução são um snapshot em memória, descartado ao encerrar/cancelar a sessão.

```json
{
  "version": 1,
  "ui_language": "pt-BR",
  "hotkeys": {
    "dictate_hold": "Ctrl+Win",
    "dictate_toggle": null,
    "double_tap_toggles": true,
    "command_hold": "Ctrl+Win+Alt",
    "voice_note_hold": "Ctrl+Win+Shift",
    "meeting_toggle": "Alt+M",
    "paste_last": "Alt+Shift+V",
    "cancel": "Escape"
  },
  "audio": {
    "input_device": "default",
    "sounds": true,
    "max_dictation_minutes": 20
  },
  "transcription": {
    "dictation_provider": "<provider_id>",
    "meeting_provider": "<provider_id>",
    "fallback_provider": "<provider_id|null>",
    "languages": ["pt", "en"],
    "language_mode": "auto"
  },
  "text": {
    "llm_provider": null,
    "cleanup_level": "light",
    "llm_timeout_ms": 3000,
    "spoken_punctuation": false
  },
  "flowbar": {
    "visibility": "always",
    "follow": "foreground_monitor",
    "position": { "edge": "bottom", "offset": 0.5 },
    "hide_in_fullscreen": true
  },
  "meetings": {
    "detect": true,
    "detect_any_call": false,
    "auto_start": false,
    "auto_stop": true,
    "max_minutes": 120,
    "live_transcript": true,
    "capture_system_audio": true
  },
  "privacy": {
    "offline_mode": false,
    "keep_dictation_audio": "24h",
    "keep_meeting_audio": "30d",
    "send_window_title": false
  },
  "system": { "launch_at_login": true, "tray": true }
}
```

## 4. Retenção

| Dado                           | Padrão                                           | Opções                                           |
| ------------------------------ | ------------------------------------------------ | ------------------------------------------------ |
| Áudio de ditado                | últimas 10 sessões ou 24 h (o que vier primeiro) | nunca · 24 h · 7 dias                            |
| Áudio de reunião               | 30 dias após `ready`                             | apagar após processar · 7 d · 30 d · para sempre |
| Transcrições, notas, histórico | até o usuário apagar                             | apagar histórico com mais de N dias              |

Uma tarefa de limpeza roda na inicialização e a cada 6 h.
