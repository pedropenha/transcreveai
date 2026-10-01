//! Etapa 5 — limpeza determinística `light` (FR-004-12, ADR-0002).
//!
//! Remove muletas pt-BR e repetições imediatas de palavra e corrige
//! pontuação e capitalização. **Não** troca palavras nem reescreve — isso é
//! `medium`/`high` (v1.1+, com LLM).
//!
//! A lista de muletas é editável na tela Dicionário (T-044 /
//! `settings.custom_filler_words`; `None` → padrão pt-BR). Dois regimes:
//!
//! - **Hesitações** ("né", "ahn", "ééé", "hmm"… — sons sem valor lexical):
//!   removidas onde quer que apareçam como token inteiro.
//! - **Muletas lexicais** ("tipo", "aí", "é", "então assim"… — também são
//!   palavras/frases reais): só saem quando **destacadas por pausas dos dois
//!   lados** (início/fim do texto ou pontuação adjacente), o que protege
//!   "um tipo de ave", "ele mora aí" e "o que é?".
//!
//! `settings.filler_removal_enabled` desliga só a remoção das entradas
//! listadas; colapso de repetições e correção de pontuação seguem ativos.

use super::token_core;
use crate::pipeline::PipelineInput;
use once_cell::sync::Lazy;
use regex::Regex;

/// Muletas pt-BR embutidas (FR-004-12). Variantes alongadas da mesma
/// hesitação ("ahnn", "éééé") são listadas explicitamente porque o match é
/// de token inteiro — a lista é literal e editável.
const DEFAULT_LIGHT_FILLER_WORDS: &[&str] = &[
    // Frases (muletas lexicais: só saem isoladas por pausas)
    "então assim",
    // Marcadores de discurso (mesmo regime: pausa dos dois lados)
    "tipo",
    "aí",
    "é",
    // Hesitações — sons sem valor lexical, saem em qualquer posição.
    // ("né" também cai aqui pelo alfabeto hesitante.)
    "né",
    "ahn",
    "ahnn",
    "ahm",
    "ahmm",
    "aan",
    "hmm",
    "hmmm",
    "hum",
    "huum",
    "mmm",
    "mmmm",
    "éé",
    "ééé",
    "éééé",
    "eee",
    "ãã",
    "ããã",
];

/// A lista de muletas padrão da limpeza `light` — exposta ao frontend via
/// `get_filler_words` para a tela Dicionário (T-044).
pub fn default_light_filler_words() -> Vec<String> {
    DEFAULT_LIGHT_FILLER_WORDS
        .iter()
        .map(|s| s.to_string())
        .collect()
}

/// Executa a limpeza `light` sobre o texto (entrada já normalizada).
pub(super) fn light(text: &str, input: &PipelineInput) -> String {
    let filler_words: Vec<String> = if input.filler_removal_enabled {
        input
            .cleanup_filler_words
            .clone()
            .unwrap_or_else(default_light_filler_words)
    } else {
        Vec::new()
    };

    let without_fillers = remove_fillers_and_repetitions(text, &filler_words);
    let fixed = fix_punctuation(&without_fillers);
    let capitalized = super::normalize::capitalize_sentence_starts(&fixed, true);
    ensure_terminal_punctuation(capitalized.trim())
}

/// Uma entrada é "hesitação" quando é um único token de 2+ letras formado só
/// por caracteres de sons hesitativos ("né", "ahn", "ééé", "hmm", "hum"…).
/// Palavras lexicais ("tipo", "aí", "é") e frases ("então assim") caem no
/// regime destacado-por-pausas.
fn is_hesitation(core: &str) -> bool {
    core.chars().count() >= 2
        && core
            .chars()
            .all(|c| matches!(c, 'a' | 'ã' | 'â' | 'e' | 'é' | 'ê' | 'h' | 'm' | 'n' | 'u'))
}

/// Pontuação que marca pausa ao lado de um token — usada para decidir se uma
/// muleta lexical está "destacada" (isolada por pausas).
fn is_pause_char(c: char) -> bool {
    matches!(
        c,
        ',' | ';' | ':' | '.' | '!' | '?' | '…' | '—' | '-' | ')' | ']' | '}' | '"' | '\'' | '»'
    )
}

fn is_terminal_char(c: char) -> bool {
    matches!(c, '.' | '!' | '?' | '…')
}

fn ends_with_pause(token: &str) -> bool {
    token.chars().last().is_some_and(is_pause_char)
}

fn ends_with_terminal(token: &str) -> bool {
    token.chars().last().is_some_and(is_terminal_char)
}

/// Pausa à esquerda do token `index`: início do texto ou token anterior
/// terminado em pontuação.
fn detached_left(tokens: &[&str], index: usize) -> bool {
    index == 0 || ends_with_pause(tokens[index - 1])
}

/// Pausa à direita do token `index`: fim do texto, pontuação no fim do
/// próprio token ou o próximo token já abrindo com pontuação.
fn detached_right(tokens: &[&str], index: usize) -> bool {
    index + 1 == tokens.len()
        || ends_with_pause(tokens[index])
        || tokens[index + 1]
            .chars()
            .next()
            .is_some_and(|c| !c.is_alphanumeric())
}

/// Remove as muletas configuradas e colapsa repetições imediatas
/// ("muito muito bom" → "muito bom"), remontando o texto sem deixar
/// pontuação órfã. Linha a linha: `\n` é fronteira natural de pausa e não
/// pode ser perdido na remontagem (AC-004-10).
fn remove_fillers_and_repetitions(text: &str, filler_words: &[String]) -> String {
    text.split('\n')
        .map(|line| clean_line(line, filler_words))
        .collect::<Vec<_>>()
        .join("\n")
}

fn clean_line(text: &str, filler_words: &[String]) -> String {
    let tokens: Vec<&str> = text.split_whitespace().collect();
    if tokens.is_empty() {
        return String::new();
    }
    let cores: Vec<String> = tokens.iter().map(|t| token_core(t)).collect();

    // Classifica as entradas configuradas: hesitações saem em qualquer
    // posição; muletas lexicais (palavra ou frase) só isoladas por pausas.
    let mut loose_words: Vec<String> = Vec::new();
    let mut detached_phrases: Vec<Vec<String>> = Vec::new();
    for entry in filler_words {
        let words: Vec<String> = entry
            .split_whitespace()
            .map(token_core)
            .filter(|w| !w.is_empty())
            .collect();
        match words.len() {
            0 => {}
            1 if is_hesitation(&words[0]) => loose_words.push(words[0].clone()),
            _ => detached_phrases.push(words),
        }
    }
    // Frases mais longas primeiro: "então assim" antes de um eventual "então".
    detached_phrases.sort_by_key(|p| std::cmp::Reverse(p.len()));

    let mut removed = vec![false; tokens.len()];

    // Muletas lexicais: só saem destacadas por pausas dos dois lados.
    for phrase in &detached_phrases {
        let n = phrase.len();
        if n > tokens.len() {
            continue;
        }
        for i in 0..=(tokens.len() - n) {
            let window_matches = (0..n).all(|j| !removed[i + j] && cores[i + j] == phrase[j]);
            if window_matches && detached_left(&tokens, i) && detached_right(&tokens, i + n - 1) {
                for j in 0..n {
                    removed[i + j] = true;
                }
            }
        }
    }

    // Hesitações (qualquer posição) + repetições imediatas.
    for (i, core) in cores.iter().enumerate() {
        if removed[i] || core.is_empty() {
            continue;
        }
        if loose_words.iter().any(|w| w == core) {
            removed[i] = true;
            continue;
        }
        // Repetição imediata da palavra anterior mantida — sem cruzar fim de
        // frase ("fim. Fim de ano" não colapsa).
        if let Some(prev) = (0..i).rev().find(|&j| !removed[j]) {
            if cores[prev] == *core && !ends_with_terminal(tokens[prev]) {
                removed[i] = true;
            }
        }
    }

    rebuild(&tokens, &removed)
}

/// Remonta o texto sem os tokens removidos. Ao tirar uma muleta, come a
/// vírgula solta que a antecedia ("pode, né, fazer" → "pode fazer") e
/// reemite a pontuação terminal que ela carregava ("tá bom né?" → "tá bom?").
fn rebuild(tokens: &[&str], removed: &[bool]) -> String {
    let mut out = String::new();
    for (i, token) in tokens.iter().enumerate() {
        if removed[i] {
            while out.ends_with(' ') {
                out.pop();
            }
            if out.ends_with(',') {
                out.pop();
            }
            // A pontuação terminal do token removido pertence à frase.
            if let Some(c) = token.chars().last().filter(|c| is_terminal_char(*c)) {
                if !out.is_empty() && !out.chars().last().is_some_and(is_terminal_char) {
                    out.push(c);
                }
            }
            continue;
        }
        if !out.is_empty() && !out.ends_with([' ', '\n']) {
            out.push(' ');
        }
        out.push_str(token);
    }
    out
}

static RE_SPACE_BEFORE_PUNCT: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"[ \t]+([,.;:!?…%)\]}])").unwrap());
static RE_PUNCT_BEFORE_TERMINAL: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"[,;:]+[ \t]*([.!?…])").unwrap());
static RE_DUP_COMMA: Lazy<Regex> = Lazy::new(|| Regex::new(r",[ \t]*[,;:]+").unwrap());
static RE_LEADING_JUNK: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?m)^[ \t]*[,;:]+[ \t]*").unwrap());
static RE_TRAILING_JUNK: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?m)[ \t]*[,;:]+[ \t]*$").unwrap());
static RE_MULTI_SPACE: Lazy<Regex> = Lazy::new(|| Regex::new(r"[ \t]{2,}").unwrap());
static RE_SPACE_AROUND_NEWLINE: Lazy<Regex> = Lazy::new(|| Regex::new(r"[ \t]*\n[ \t]*").unwrap());

/// Corrige pontuação depois das remoções: espaço antes de pontuação, sinais
/// orfãos na frente de terminal (", ." → "."), vírgulas dobradas, pausas
/// soltas no início/fim de linha e espaços duplicados.
fn fix_punctuation(text: &str) -> String {
    let t = RE_SPACE_BEFORE_PUNCT.replace_all(text, "$1");
    let t = RE_PUNCT_BEFORE_TERMINAL.replace_all(&t, "$1");
    let t = RE_DUP_COMMA.replace_all(&t, ",");
    let t = RE_LEADING_JUNK.replace_all(&t, "");
    let t = RE_TRAILING_JUNK.replace_all(&t, "");
    let t = RE_SPACE_AROUND_NEWLINE.replace_all(&t, "\n");
    RE_MULTI_SPACE.replace_all(&t, " ").to_string()
}

/// O texto contém ideogramas CJK (transcrição zh/ja) — nesse caso o "." do
/// latim não é a pontuação certa ("。") e melhor não inventar nada.
fn has_cjk_ideograph(text: &str) -> bool {
    text.chars()
        .any(|c| ('\u{4E00}'..='\u{9FFF}').contains(&c) || ('\u{3400}'..='\u{4DBF}').contains(&c))
}

/// "Corrige pontuação": texto de linha única sem pontuação final ganha ".".
/// Texto já quebrado por comandos de voz não recebe ponto sintético — as
/// quebras já estruturam o ditado (AC-004-10) — nem texto CJK.
fn ensure_terminal_punctuation(text: &str) -> String {
    if text.contains('\n') || text.is_empty() || has_cjk_ideograph(text) {
        return text.to_string();
    }
    match text.chars().last() {
        Some(c) if c.is_alphanumeric() => format!("{text}."),
        _ => text.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::CleanupLevel;

    fn light_of(text: &str) -> String {
        light(
            text,
            &PipelineInput {
                text: text.to_string(),
                cleanup_level: CleanupLevel::Light,
                ..Default::default()
            },
        )
    }

    // --- Hesitações: removidas em qualquer posição -------------------------

    #[test]
    fn hesitation_mid_sentence_is_removed() {
        assert_eq!(light_of("eu ahn não sei"), "Eu não sei.");
        assert_eq!(light_of("eu ééé não sei"), "Eu não sei.");
        assert_eq!(light_of("isso hmm ficou"), "Isso ficou.");
    }

    #[test]
    fn ne_tag_is_removed_and_keeps_terminal_punctuation() {
        assert_eq!(light_of("tá bom né?"), "Tá bom?");
        assert_eq!(light_of("vamos fazer, né, amanhã"), "Vamos fazer amanhã.");
        assert_eq!(light_of("tá bom né"), "Tá bom.");
    }

    /// Muleta lexical no início de frase, seguida de vírgula: destacada, sai.
    #[test]
    fn lexical_filler_at_sentence_start_is_removed() {
        assert_eq!(light_of("é, eu não sei"), "Eu não sei.");
        assert_eq!(light_of("tipo, eu acho"), "Eu acho.");
        assert_eq!(light_of("aí, vem cá"), "Vem cá.");
    }

    /// Muleta lexical entre pausas no meio da frase: sai junto com a pausa.
    #[test]
    fn lexical_filler_between_pauses_is_removed() {
        assert_eq!(
            light_of("a gente pode, né, fazer amanhã"),
            "A gente pode fazer amanhã."
        );
        assert_eq!(light_of("isso, tipo, funcionou"), "Isso funcionou.");
        assert_eq!(light_of("ele chegou, aí, e viu"), "Ele chegou e viu.");
    }

    /// Palavras reais iguais às muletas sobrevivem quando não estão isoladas:
    /// "um tipo de ave", "ele mora aí", "o que é?".
    #[test]
    fn real_words_matching_fillers_survive() {
        assert_eq!(light_of("um tipo de ave"), "Um tipo de ave.");
        assert_eq!(light_of("ele mora aí"), "Ele mora aí.");
        assert_eq!(light_of("tá legal aí?"), "Tá legal aí?");
        assert_eq!(light_of("o que é isso"), "O que é isso.");
        assert_eq!(light_of("o que é?"), "O que é?");
        assert_eq!(light_of("não é verdade"), "Não é verdade.");
    }

    /// Frase muleta "então assim" sai isolada por pausas.
    #[test]
    fn phrase_filler_is_removed() {
        assert_eq!(light_of("vamos, então assim, decidir"), "Vamos decidir.");
        assert_eq!(light_of("então assim, ficou pronto"), "Ficou pronto.");
    }

    /// ...mas a mesma frase no meio da frase (sem pausas) é preservada.
    #[test]
    fn phrase_filler_without_pauses_survives() {
        assert_eq!(
            light_of("a ordem era então assim cumprida"),
            "A ordem era então assim cumprida."
        );
    }

    // --- Repetições imediatas ----------------------------------------------

    #[test]
    fn immediate_repetition_collapses() {
        assert_eq!(light_of("muito muito bom"), "Muito bom.");
        assert_eq!(light_of("eu eu acho"), "Eu acho.");
        assert_eq!(light_of("eu, eu acho"), "Eu acho.");
        assert_eq!(light_of("não não não"), "Não.");
    }

    /// Repetição legítima separada por fim de frase não colapsa.
    #[test]
    fn repetition_across_sentence_boundary_is_kept() {
        assert_eq!(light_of("acabou. Fim de semana"), "Acabou. Fim de semana.");
    }

    /// Palavra que se repete mais adiante (não imediata) é preservada.
    #[test]
    fn non_adjacent_repetition_is_kept() {
        assert_eq!(light_of("eu vi eu mesmo"), "Eu vi eu mesmo.");
    }

    // --- Pontuação e capitalização -----------------------------------------

    #[test]
    fn fixes_space_before_punctuation() {
        assert_eq!(light_of("oi , tudo bem ?"), "Oi, tudo bem?");
        assert_eq!(light_of("sim , claro"), "Sim, claro.");
    }

    #[test]
    fn merges_dangling_comma_before_terminal() {
        assert_eq!(light_of("falei, né."), "Falei.");
        assert_eq!(light_of("chegou , !"), "Chegou!");
    }

    #[test]
    fn strips_leading_pause_junk() {
        assert_eq!(light_of(", isso aí tudo certo"), "Isso aí tudo certo.");
    }

    #[test]
    fn capitalizes_after_terminal_punctuation() {
        assert_eq!(light_of("falei. depois fui."), "Falei. Depois fui.");
        assert_eq!(light_of("sério? juro."), "Sério? Juro.");
    }

    /// Ponto interno de palavra ("transcreve.ai", "3.5") não é fronteira de
    /// frase — a capitalização exige espaço após o terminal.
    #[test]
    fn inner_period_is_not_a_sentence_boundary() {
        assert_eq!(
            light_of("eu uso o transcreve.ai todo dia"),
            "Eu uso o transcreve.ai todo dia."
        );
    }

    #[test]
    fn appends_terminal_period_only_when_missing() {
        assert_eq!(light_of("vou chegar em cinco"), "Vou chegar em cinco.");
        assert_eq!(light_of("vai dar certo!"), "Vai dar certo!");
    }

    /// Texto já quebrado por comando de voz não ganha ponto sintético
    /// (AC-004-10).
    #[test]
    fn multiline_text_gets_no_synthetic_period() {
        assert_eq!(
            light_of("Primeira linha\nSegunda linha"),
            "Primeira linha\nSegunda linha"
        );
    }

    /// Texto CJK não recebe "." do latim.
    #[test]
    fn cjk_text_gets_no_synthetic_period() {
        assert_eq!(light_of("这是测试"), "这是测试");
    }

    #[test]
    fn empty_input_stays_empty() {
        assert_eq!(light_of(""), "");
        assert_eq!(light_of("   "), "");
    }

    // --- Lista editável e switches ------------------------------------------

    /// FR-004-12: a lista de muletas é editável — entradas customizadas se
    /// aplicam com o mesmo regime destacado-por-pausas.
    #[test]
    fn custom_filler_list_overrides_defaults() {
        let input = PipelineInput {
            text: "isso, cara, funcionou".to_string(),
            cleanup_filler_words: Some(vec!["cara".to_string()]),
            cleanup_level: CleanupLevel::Light,
            ..Default::default()
        };
        assert_eq!(light("isso, cara, funcionou", &input), "Isso funcionou.");
        // O padrão não se aplica quando a lista foi substituída.
        assert_eq!(light("isso, né, funcionou", &input), "Isso, né, funcionou.");
    }

    /// Lista vazia: nenhuma muleta sai, mas pontuação/capitalização seguem.
    #[test]
    fn empty_custom_list_keeps_fillers_but_fixes_punctuation() {
        let input = PipelineInput {
            cleanup_filler_words: Some(Vec::new()),
            cleanup_level: CleanupLevel::Light,
            ..Default::default()
        };
        assert_eq!(light("isso, né, funcionou", &input), "Isso, né, funcionou.");
        // Repetição imediata ainda colapsa — não é muleta "listada".
        assert_eq!(light("muito muito bom", &input), "Muito bom.");
    }

    /// `filler_word_removal_enabled = false` preserva as muletas listadas.
    #[test]
    fn disabled_filler_removal_keeps_fillers() {
        let input = PipelineInput {
            filler_removal_enabled: false,
            cleanup_level: CleanupLevel::Light,
            ..Default::default()
        };
        assert_eq!(light("tá bom né?", &input), "Tá bom né?");
    }

    /// Muleta com pontuação colada no próprio token ("né…", "(tipo)").
    #[test]
    fn muleta_with_adjacent_punctuation_forms() {
        assert_eq!(light_of("isso, né… vai"), "Isso… Vai.");
        assert_eq!(light_of("(tipo) estranho"), "Estranho.");
    }
}
