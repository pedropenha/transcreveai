//! Etapa 1 — normalização (FR-004-01): trim, colapso de espaços e
//! capitalização da primeira letra (do texto e de cada linha).
//!
//! "Remover segmentos filtrados como alucinação" acontece a montante, no
//! pós-processamento do motor (`managers::transcription::postprocess` /
//! FR-003-13), que decide com a energia do áudio — fora do alcance de uma
//! função pura de texto.

/// Normaliza o texto cru: colapsa espaços horizontais, remove espaços ao
/// redor de quebras de linha (preservando `\n` e `\n\n`), faz trim e
/// capitaliza a primeira letra de cada linha.
pub(super) fn run(text: &str) -> String {
    let collapsed = collapse_whitespace(text);
    capitalize_sentence_starts(&collapsed, false)
}

/// Colapsa espaços/tabs em um único espaço e remove espaços adjacentes a
/// `\n`. Não toca nas próprias quebras de linha.
fn collapse_whitespace(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut pending_space = false;
    for c in text.chars() {
        match c {
            '\n' => {
                out.push('\n');
                pending_space = false;
            }
            ' ' | '\t' | '\r' => pending_space = true,
            _ => {
                if pending_space && !out.is_empty() && !out.ends_with('\n') {
                    out.push(' ');
                }
                pending_space = false;
                out.push(c);
            }
        }
    }
    out.trim().to_string()
}

/// Caracteres que podem preceder a primeira letra de uma frase sem "gastar" a
/// posição de início (aspas e parênteses de abertura).
fn is_sentence_open_char(c: char) -> bool {
    matches!(c, '"' | '\'' | '(' | '[' | '{' | '“' | '‘' | '«' | '‹')
}

/// Pontuação que encerra uma frase (para a capitalização da etapa 5).
fn is_terminal_char(c: char) -> bool {
    matches!(c, '.' | '!' | '?' | '…')
}

/// Capitaliza a primeira letra após cada `\n` — e, quando
/// `after_punctuation` é verdade (etapa 5), após `.` `!` `?` `…` **seguidos
/// de espaço**. A exigência do espaço evita reescrever pontos internos
/// ("Transcreve.ai", "ex.:", "3.5"). Espaços e sinais de abertura (`"`, `(`,
/// `«`…) não consomem a posição de início de frase; um dígito ou outro
/// caractere encerra a espera ("3 pontos" não vira "3 Pontos").
pub(crate) fn capitalize_sentence_starts(text: &str, after_punctuation: bool) -> String {
    let mut out = String::with_capacity(text.len());
    let mut pending_capital = true;
    let mut terminal_seen = false;
    for c in text.chars() {
        if pending_capital && c.is_alphabetic() {
            out.extend(c.to_uppercase());
            pending_capital = false;
            terminal_seen = false;
            continue;
        }
        out.push(c);
        if c == '\n' {
            pending_capital = true;
            terminal_seen = false;
        } else if after_punctuation && is_terminal_char(c) {
            terminal_seen = true;
        } else if terminal_seen && c.is_whitespace() {
            pending_capital = true;
            terminal_seen = false;
        } else if !c.is_whitespace() && !is_sentence_open_char(c) {
            pending_capital = false;
            terminal_seen = false;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trims_and_collapses_spaces() {
        assert_eq!(run("  oi   tudo   bem  "), "Oi tudo bem");
        assert_eq!(run("\to la\n"), "O la");
    }

    /// FR-004-01: capitaliza a primeira letra.
    #[test]
    fn capitalizes_first_letter() {
        assert_eq!(run("olá mundo"), "Olá mundo");
        assert_eq!(run("é isso"), "É isso");
    }

    #[test]
    fn capitalizes_first_letter_of_each_line() {
        assert_eq!(run("primeira\nsegunda"), "Primeira\nSegunda");
        assert_eq!(run("um\n\ndois"), "Um\n\nDois");
    }

    #[test]
    fn keeps_line_breaks_and_drops_spaces_around_them() {
        assert_eq!(run("a  \n  b"), "A\nB");
        assert_eq!(run("a\n\n\nb"), "A\n\n\nB");
    }

    #[test]
    fn quotes_and_parens_do_not_consume_the_sentence_start() {
        assert_eq!(run("\"olá\""), "\"Olá\"");
        assert_eq!(run("«olá»"), "«Olá»");
    }

    #[test]
    fn leading_digit_is_not_a_capitalizable_letter() {
        assert_eq!(run("3 pontos seguidos"), "3 pontos seguidos");
    }

    #[test]
    fn sentence_capitalization_after_punctuation() {
        assert_eq!(
            capitalize_sentence_starts("falei. depois fui. e aí?", true),
            "Falei. Depois fui. E aí?"
        );
        assert_eq!(
            capitalize_sentence_starts("falei. depois fui", false),
            "Falei. depois fui"
        );
    }
}
