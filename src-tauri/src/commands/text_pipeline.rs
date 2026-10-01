//! Comandos IPC do pipeline de texto determinístico (F004 / T-035).
//!
//! Ponte para a tela Dicionário (T-044): expõe a lista de muletas da limpeza
//! `light` (FR-004-12), o nível de limpeza (FR-004-11) e a pontuação falada
//! (FR-004-03). Tudo persiste em `AppSettings` via `write_settings`.

use crate::commands::{CommandError, CommandErrorCode, CommandResult};
use crate::pipeline::{self, CleanupLevel};
use crate::settings;
use tauri::AppHandle;

/// Limites defensivos para a lista editável de muletas.
const MAX_FILLER_WORDS: usize = 500;
const MAX_FILLER_WORD_LEN: usize = 60;

/// A lista de muletas efetiva da limpeza `light`: a editada pelo usuário
/// (`custom_filler_words`) ou, quando ausente, o padrão pt-BR embutido.
#[tauri::command]
#[specta::specta]
pub fn get_filler_words(app: AppHandle) -> CommandResult<Vec<String>> {
    let settings = settings::get_settings(&app);
    Ok(settings
        .custom_filler_words
        .clone()
        .unwrap_or_else(pipeline::default_light_filler_words))
}

/// Persiste a lista editável de muletas (FR-004-12). Entradas são
/// normalizadas (trim, minúsculas, sem duplicatas nem vazias). Uma lista
/// vazia é válida: desliga a remoção de muletas sem mexer no resto da
/// limpeza `light` (repetições, pontuação, capitalização).
#[tauri::command]
#[specta::specta]
pub fn set_filler_words(app: AppHandle, words: Vec<String>) -> CommandResult<()> {
    if words.len() > MAX_FILLER_WORDS {
        return Err(CommandError::new(
            CommandErrorCode::InvalidInput,
            format!("filler word list exceeds the {MAX_FILLER_WORDS}-entry limit"),
        ));
    }

    let mut cleaned: Vec<String> = Vec::with_capacity(words.len());
    for word in &words {
        let normalized = word.split_whitespace().collect::<Vec<_>>().join(" ");
        if normalized.is_empty() {
            continue;
        }
        if normalized.chars().count() > MAX_FILLER_WORD_LEN {
            return Err(CommandError::new(
                CommandErrorCode::InvalidInput,
                format!("filler word exceeds the {MAX_FILLER_WORD_LEN}-character limit"),
            ));
        }
        let lowered = normalized.to_lowercase();
        if !cleaned.contains(&lowered) {
            cleaned.push(lowered);
        }
    }

    let mut settings = settings::get_settings(&app);
    settings.custom_filler_words = Some(cleaned);
    settings::write_settings(&app, settings);
    Ok(())
}

/// Volta a lista de muletas ao padrão pt-BR embutido (`custom_filler_words`
/// → `None`).
#[tauri::command]
#[specta::specta]
pub fn reset_filler_words(app: AppHandle) -> CommandResult<()> {
    let mut settings = settings::get_settings(&app);
    settings.custom_filler_words = None;
    settings::write_settings(&app, settings);
    Ok(())
}

/// Nível de limpeza do pipeline (FR-004-11). Na v1 a UI só oferece
/// `none`/`light`; `medium`/`high` (v1.1+, LLM) são aceitos aqui mas
/// degradam para `light` no pipeline.
#[tauri::command]
#[specta::specta]
pub fn change_cleanup_level_setting(app: AppHandle, level: CleanupLevel) -> CommandResult<()> {
    let mut settings = settings::get_settings(&app);
    settings.cleanup_level = level;
    settings::write_settings(&app, settings);
    Ok(())
}

/// Liga/desliga a pontuação falada ("vírgula" → `,`), desligada por padrão
/// (FR-004-03).
#[tauri::command]
#[specta::specta]
pub fn change_spoken_punctuation_setting(app: AppHandle, enabled: bool) -> CommandResult<()> {
    let mut settings = settings::get_settings(&app);
    settings.spoken_punctuation_enabled = enabled;
    settings::write_settings(&app, settings);
    Ok(())
}
