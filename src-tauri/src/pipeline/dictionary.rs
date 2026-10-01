//! Etapa 4 — dicionário, entradas `vocab` (FR-004-08).
//!
//! Na v1 as entradas `vocab` já foram ao STT como dicas de vocabulário
//! (FR-003-12, `managers::transcription::prompt`); aqui reaplicamos a
//! correção fuzzy determinística de `audio_toolkit::text` — idempotente para
//! texto que o motor já acertou (casa exato → mesma grafia) e útil quando o
//! hint não bastou ou o modelo não recebe prompt (não-Whisper). Entradas
//! `replacement` chegam na v1.1+ (FR-004-09).

/// Aplica a correção fuzzy das entradas `vocab` sobre o texto.
/// No-op quando a lista está vazia.
pub(super) fn run(text: &str, custom_words: &[String], threshold: f64) -> String {
    crate::audio_toolkit::apply_custom_words(text, custom_words, threshold)
}
