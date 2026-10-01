//! Etapa 2 — comandos de voz determinísticos (FR-004-02/03).
//!
//! - "nova linha" / "new line" → `\n`; "novo parágrafo" / "new paragraph" →
//!   `\n\n`, com a letra seguinte capitalizada;
//! - "enviar" / "send" / "press enter" **só no final** do ditado → sai do
//!   texto e sobe a flag `press_enter` (o F002 envia a tecla após a inserção);
//! - pontuação falada ("vírgula", "ponto final", "question mark") é
//!   **desligada por padrão** (FR-004-03) — os motores já pontuam.
//!
//! As frases são configuráveis por idioma: `"default"` sempre vale, a chave
//! do idioma efetivo e seu prefixo somam (o `pt` cobre `pt-BR`).

use super::token_core;
use crate::pipeline::{PipelineInput, VoiceCommandPhrases};
use once_cell::sync::Lazy;
use std::collections::HashMap;

/// Item do fluxo de tokens da etapa 2: palavra comum, quebra inserida por
/// comando ou sinal de pontuação falada (colado na palavra anterior).
enum Item {
    Word(String),
    Newline,
    Paragraph,
    Punct(&'static str),
}

/// O que uma frase de comando produz — `Item::Word` nunca é alvo.
#[derive(Clone, Copy)]
enum Target {
    Newline,
    Paragraph,
    Punct(&'static str),
}

impl Target {
    fn into_item(self) -> Item {
        match self {
            Target::Newline => Item::Newline,
            Target::Paragraph => Item::Paragraph,
            Target::Punct(symbol) => Item::Punct(symbol),
        }
    }
}

/// Palavras normalizadas de uma frase ("Nova Linha" → ["nova", "linha"]).
fn phrase_words(phrase: &str) -> Vec<String> {
    phrase.split_whitespace().map(token_core).collect()
}

/// Resolve as frases de comando aplicáveis ao idioma efetivo: "default"
/// sempre, mais a chave exata ("pt-br") e o prefixo ("pt"). Idioma
/// desconhecido/`auto` fica só com "default".
fn resolve_command_phrases(
    map: &HashMap<String, VoiceCommandPhrases>,
    language: Option<&str>,
) -> VoiceCommandPhrases {
    let lang_tag = language.unwrap_or_default().to_lowercase();
    let lang_prefix = lang_tag.split(['-', '_']).next().unwrap_or_default();
    let mut resolved = VoiceCommandPhrases::default();
    for key in ["default", lang_tag.as_str(), lang_prefix] {
        if key.is_empty() {
            continue;
        }
        if let Some(entry) = map.get(key) {
            resolved.newline.extend(entry.newline.iter().cloned());
            resolved
                .new_paragraph
                .extend(entry.new_paragraph.iter().cloned());
        }
    }
    resolved.newline.sort();
    resolved.newline.dedup();
    resolved.new_paragraph.sort();
    resolved.new_paragraph.dedup();
    resolved
}

/// Pontuação falada (FR-004-03): frase → sinal, por idioma. Ordenada por
/// tamanho na aplicação (mais longa primeiro, "ponto de interrogação" antes
/// de "interrogação").
fn spoken_punctuation_table(base_lang: &str) -> &'static [(&'static str, &'static str)] {
    match base_lang {
        "pt" => &[
            ("ponto de interrogação", "?"),
            ("ponto de exclamação", "!"),
            ("ponto e vírgula", ";"),
            ("ponto final", "."),
            ("dois pontos", ":"),
            ("reticências", "…"),
            ("vírgula", ","),
            ("interrogação", "?"),
            ("exclamação", "!"),
        ],
        _ => &[
            ("question mark", "?"),
            ("exclamation mark", "!"),
            ("exclamation point", "!"),
            ("semicolon", ";"),
            ("full stop", "."),
            ("period", "."),
            ("colon", ":"),
            ("ellipsis", "…"),
            ("comma", ","),
        ],
    }
}

/// Tabelas de pontuação falada combinadas para quando o idioma da saída é
/// desconhecido (`auto`): os tokens são específicos de cada idioma, então
/// cruzar as tabelas não gera falso positivo.
static SPOKEN_PUNCT_ALL: Lazy<Vec<(&'static str, &'static str)>> = Lazy::new(|| {
    let mut all: Vec<(&'static str, &'static str)> = Vec::new();
    all.extend_from_slice(spoken_punctuation_table("pt"));
    all.extend_from_slice(spoken_punctuation_table("en"));
    all
});

/// Executa a etapa 2 sobre o texto normalizado. Devolve o texto reescrito e
/// `press_enter` quando um comando de envio foi reconhecido no final.
pub(super) fn run(text: &str, input: &PipelineInput) -> (String, bool) {
    if text.trim().is_empty() {
        return (text.to_string(), false);
    }

    let commands = resolve_command_phrases(&input.voice_command_phrases, input.language.as_deref());

    // Alvos (frase → item): quebras sempre; pontuação falada só se ligada.
    let mut targets: Vec<(Vec<String>, Target)> = Vec::new();
    for phrase in &commands.newline {
        let words = phrase_words(phrase);
        if !words.is_empty() {
            targets.push((words, Target::Newline));
        }
    }
    for phrase in &commands.new_paragraph {
        let words = phrase_words(phrase);
        if !words.is_empty() {
            targets.push((words, Target::Paragraph));
        }
    }
    if input.spoken_punctuation_enabled {
        let lang_tag = input.language.as_deref().unwrap_or_default().to_lowercase();
        let base_lang = lang_tag.split(['-', '_']).next().unwrap_or_default();
        let table: &[(&'static str, &'static str)] = match base_lang {
            // Idioma conhecido: só a tabela dele. Desconhecido/auto: união —
            // os tokens ("vírgula", "comma") são específicos de cada idioma.
            "pt" | "en" => spoken_punctuation_table(base_lang),
            _ => &SPOKEN_PUNCT_ALL,
        };
        for (phrase, symbol) in table {
            let words = phrase_words(phrase);
            if !words.is_empty() {
                targets.push((words, Target::Punct(symbol)));
            }
        }
    }
    // Frases mais longas primeiro: "novo parágrafo" não deve ser comido por
    // um alvo de uma palavra, "ponto de interrogação" ganha de "interrogação".
    targets.sort_by_key(|(words, _)| std::cmp::Reverse(words.len()));

    let tokens: Vec<String> = text.split_whitespace().map(|t| t.to_string()).collect();
    let cores: Vec<String> = tokens.iter().map(|t| token_core(t)).collect();

    let mut items: Vec<Item> = Vec::with_capacity(tokens.len());
    let mut i = 0;
    while i < tokens.len() {
        let mut matched = false;
        if !targets.is_empty() {
            for (words, kind) in &targets {
                let n = words.len();
                if i + n <= tokens.len() && (0..n).all(|j| cores[i + j] == words[j]) {
                    items.push(kind.into_item());
                    i += n;
                    matched = true;
                    break;
                }
            }
        }
        if !matched {
            items.push(Item::Word(tokens[i].clone()));
            i += 1;
        }
    }

    let rebuilt = join_items(&items);
    // A frase após uma quebra inserida por comando abre "frase" — capitaliza.
    let rebuilt = super::normalize::capitalize_sentence_starts(&rebuilt, false);

    // FR-004-02: "enviar" só conta no final do ditado.
    let language = input.language.as_deref().unwrap_or_default();
    match strip_trailing_submit_command(&rebuilt, language, &input.voice_submit_phrases) {
        Some(cleaned) => (cleaned, true),
        None => (rebuilt, false),
    }
}

/// Remonta o texto a partir dos itens: quebras produzem `\n` real e sinais
/// de pontuação colam na palavra anterior ("oi , tudo" → "oi, tudo").
fn join_items(items: &[Item]) -> String {
    let mut out = String::new();
    for item in items {
        match item {
            Item::Newline => {
                while out.ends_with(' ') {
                    out.pop();
                }
                if !out.is_empty() {
                    out.push('\n');
                }
            }
            Item::Paragraph => {
                while out.ends_with(' ') {
                    out.pop();
                }
                if !out.is_empty() {
                    out.push_str("\n\n");
                }
            }
            Item::Punct(symbol) => {
                while out.ends_with(' ') {
                    out.pop();
                }
                out.push_str(symbol);
            }
            Item::Word(word) => {
                if !out.is_empty() && !out.ends_with('\n') {
                    out.push(' ');
                }
                out.push_str(word);
            }
        }
    }
    out
}

/// FR-002-17 / AC-002-10 (movido de `actions.rs` para a etapa 2 do pipeline,
/// FR-004-02): um comando falado de "enviar" como últimas palavras do ditado
/// ("… enviar", "… send", "… press enter") é um comando de sessão, não texto:
/// é removido e a flag `press_enter` dispara a tecla de envio após a inserção.
/// Devolve o texto limpo quando um comando no final foi encontrado; `None`
/// caso contrário (ou quando remover deixaria o texto vazio).
///
/// As frases vêm de `settings.voice_submit_phrases`: a lista `"default"`
/// sempre vale e a chave de idioma cobre o idioma correspondente — `"pt"`
/// também cobre `"pt-BR"`.
pub fn strip_trailing_submit_command(
    text: &str,
    language: &str,
    phrases: &HashMap<String, Vec<String>>,
) -> Option<String> {
    let tail = text.trim_end();
    if tail.is_empty() {
        return None;
    }
    // Keep the terminal punctuation the model produced so the stripped text
    // still ends a sentence ("… minutos enviar." → "… minutos.").
    let terminal = tail.chars().last().filter(|c| matches!(c, '.' | '!' | '?'));
    let tail = tail.trim_end_matches(['.', '!', '?', ',', ';', ':', '…', '"', '\'', ')', ']']);
    if tail.is_empty() {
        return None;
    }

    // Candidate phrases: longest first so "press enter" wins over "enter".
    let lang_tag = language.to_lowercase();
    let lang_prefix = lang_tag.split(['-', '_']).next().unwrap_or("");
    let mut candidates: Vec<String> = Vec::new();
    for key in ["default", lang_tag.as_str(), lang_prefix] {
        if key.is_empty() {
            continue;
        }
        if let Some(list) = phrases.get(key) {
            candidates.extend(list.iter().map(|p| p.trim().to_lowercase()));
        }
    }
    candidates.sort_by_key(|p| std::cmp::Reverse(p.chars().count()));
    candidates.dedup();

    let tail_lower_chars: Vec<char> = tail.to_lowercase().chars().collect();
    for phrase in candidates {
        if phrase.is_empty() {
            continue;
        }
        let phrase_chars: Vec<char> = phrase.chars().collect();
        if !tail_lower_chars.ends_with(&phrase_chars) {
            continue;
        }
        // The command must start on a word boundary — preceded by whitespace
        // or at the very start of the dictation.
        let boundary = tail_lower_chars.len() - phrase_chars.len();
        if boundary > 0 && !tail_lower_chars[boundary - 1].is_whitespace() {
            continue;
        }
        let strip_chars = tail.chars().count() - phrase_chars.len();
        let mut cleaned: String = tail.chars().take(strip_chars).collect();
        cleaned = cleaned.trim_end().to_string();
        if cleaned.is_empty() {
            return None;
        }
        if let Some(punct) = terminal {
            if !cleaned.ends_with(['.', '!', '?']) {
                cleaned.push(punct);
            }
        }
        return Some(cleaned);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::{default_voice_submit_phrases, CleanupLevel};

    fn input(text: &str, language: Option<&str>) -> PipelineInput {
        PipelineInput {
            text: text.to_string(),
            language: language.map(|l| l.to_string()),
            cleanup_level: CleanupLevel::None, // isola a etapa 2
            ..Default::default()
        }
    }

    /// FR-004-02 / AC-004-10: "nova linha" vira `\n`.
    #[test]
    fn nova_linha_inserts_newline() {
        let (out, enter) = run(
            "Primeira linha nova linha segunda linha",
            &input("x", Some("pt-BR")),
        );
        assert_eq!(out, "Primeira linha\nSegunda linha");
        assert!(!enter);
    }

    /// FR-004-02: "novo parágrafo" vira `\n\n`.
    #[test]
    fn novo_paragrafo_inserts_paragraph_break() {
        let (out, _) = run(
            "fim do assunto novo parágrafo outro assunto",
            &input("x", Some("pt")),
        );
        assert_eq!(out, "Fim do assunto\n\nOutro assunto");
    }

    /// FR-004-02: tabela `en` ("new line") com idioma inglês.
    #[test]
    fn english_newline_command() {
        let (out, _) = run("first line new line second line", &input("x", Some("en")));
        assert_eq!(out, "First line\nSecond line");
    }

    /// Idioma desconhecido/auto: só a tabela `"default"` — que já cobre
    /// "nova linha" e "new line".
    #[test]
    fn default_commands_apply_without_language() {
        let (out, _) = run("primeira nova linha segunda", &input("x", None));
        assert_eq!(out, "Primeira\nSegunda");
    }

    /// O comando não combate dentro de palavra ("novamente" ≠ "nova").
    #[test]
    fn commands_require_whole_words() {
        let (out, _) = run("a novamente linha ficou", &input("x", Some("pt")));
        assert_eq!(out, "A novamente linha ficou");
    }

    /// FR-004-02: "enviar" só vale no final — no meio é texto comum.
    #[test]
    fn send_command_only_at_the_end() {
        let (out, enter) = run("vou chegar em cinco enviar", &input("x", Some("pt")));
        assert_eq!(out, "Vou chegar em cinco");
        assert!(enter);

        let (out, enter) = run("enviar isso depois", &input("x", Some("pt")));
        assert_eq!(out, "Enviar isso depois");
        assert!(!enter);
    }

    /// Pontuação terminal do ditado é preservada ao tirar o "enviar".
    #[test]
    fn send_command_keeps_terminal_punctuation() {
        let (out, enter) = run(
            "vou chegar em cinco minutos enviar.",
            &input("x", Some("pt-BR")),
        );
        assert_eq!(out, "Vou chegar em cinco minutos.");
        assert!(enter);
    }

    /// Ditado que é só "enviar" não vira Enter solto: texto fica.
    #[test]
    fn bare_send_command_is_not_stripped() {
        let (out, enter) = run("enviar", &input("x", Some("pt")));
        assert_eq!(out, "Enviar");
        assert!(!enter);
    }

    /// Comando de envio depois de "nova linha": a quebra entra e o enviar sai.
    #[test]
    fn newline_then_send() {
        let (out, enter) = run("manda um oi nova linha enviar", &input("x", Some("pt")));
        assert_eq!(out, "Manda um oi");
        assert!(enter);
    }

    /// FR-004-03: pontuação falada desligada por padrão.
    #[test]
    fn spoken_punctuation_off_by_default() {
        let (out, _) = run("oi vírgula tudo bem", &input("x", Some("pt")));
        assert_eq!(out, "Oi vírgula tudo bem");
    }

    /// FR-004-03: ligada, "vírgula" vira `,` colada na palavra anterior.
    #[test]
    fn spoken_punctuation_enabled() {
        let mut i = input("x", Some("pt"));
        i.spoken_punctuation_enabled = true;
        let (out, _) = run("oi vírgula tudo bem ponto final", &i);
        assert_eq!(out, "Oi, tudo bem.");
        let (out, _) = run("é isso ponto de interrogação", &i);
        assert_eq!(out, "É isso?");
    }

    /// Pontuação falada em inglês com a tabela `en`.
    #[test]
    fn spoken_punctuation_english() {
        let mut i = input("x", Some("en"));
        i.spoken_punctuation_enabled = true;
        let (out, _) = run("hi comma how are you question mark", &i);
        assert_eq!(out, "Hi, how are you?");
    }

    // -- strip_trailing_submit_command (movido de actions.rs, AC-002-10) --

    fn voice_phrases() -> HashMap<String, Vec<String>> {
        default_voice_submit_phrases()
    }

    /// AC-002-10: "vou chegar em 5 minutos enviar" insere "Vou chegar em 5
    /// minutos." e enfileira a tecla de envio.
    #[test]
    fn voice_send_command_is_stripped_and_punctuation_kept() {
        let phrases = voice_phrases();
        assert_eq!(
            strip_trailing_submit_command("Vou chegar em 5 minutos enviar.", "pt-BR", &phrases),
            Some("Vou chegar em 5 minutos.".to_string())
        );
        assert_eq!(
            strip_trailing_submit_command("Vou chegar em 5 minutos enviar", "pt", &phrases),
            Some("Vou chegar em 5 minutos".to_string())
        );
        assert_eq!(
            strip_trailing_submit_command("see you soon send", "en", &phrases),
            Some("see you soon".to_string())
        );
        // The default list applies to every language.
        assert_eq!(
            strip_trailing_submit_command("vou chegar enviar", "auto", &phrases),
            Some("vou chegar".to_string())
        );
    }

    #[test]
    fn voice_send_command_requires_trailing_word_boundary() {
        let phrases = voice_phrases();
        // Not at the end of the dictation — plain dictation text.
        assert_eq!(
            strip_trailing_submit_command("enviar isso depois", "pt", &phrases),
            None
        );
        // Mid-word: "enviarei" must not match "enviar".
        assert_eq!(
            strip_trailing_submit_command("vou enviarei", "pt", &phrases),
            None
        );
        // No command at all.
        assert_eq!(
            strip_trailing_submit_command("vou chegar em 5 minutos", "pt", &phrases),
            None
        );
        // A bare command word leaves nothing to insert.
        assert_eq!(
            strip_trailing_submit_command("enviar", "pt", &phrases),
            None
        );
        assert_eq!(strip_trailing_submit_command("", "pt", &phrases), None);
    }

    #[test]
    fn voice_send_command_prefers_longest_phrase() {
        let phrases = voice_phrases();
        assert_eq!(
            strip_trailing_submit_command("finish now press enter", "en", &phrases),
            Some("finish now".to_string())
        );
    }
}
