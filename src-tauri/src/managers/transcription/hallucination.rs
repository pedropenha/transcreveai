//! Filtro de alucinação do STT local (FR-003-13, AC-003-05).
//!
//! Whisper-family decoders produzem texto-fantasma sobre silêncio/ruído —
//! boilerplate de legendas do corpus de treino ("Obrigado por assistir",
//! "Legendas pela comunidade Amara.org", "Thanks for watching", tags tipo
//! `[Música]`) — e entram em loops degenerados ("Thank you." × N). Este módulo
//! é a segunda linha de defesa: a primeira é o VAD na captura (FR-003-11 /
//! T-011 — quando o VAD não ouve fala, o provider nem é chamado); aqui fica o
//! caso do VAD desligado ou do ruído que passou pelo VAD.
//!
//! Regras determinísticas, na ordem:
//!
//! * **R1 — saída toda-fantasma**: se o texto normalizado é composto só de
//!   frases da lista de bloqueio → descarta quando a energia do trecho é baixa
//!   (`< LOW_ENERGY_RMS`) **ou** desconhecida (caminho de streaming, onde não
//!   há amostras). Com energia de fala normal, a frase pode ser ditado real
//!   ("obrigado por assistir" num roteiro de vídeo) e é mantida — é o "e/ou
//!   energia baixa" da spec, já que a v1 não expõe `no_speech_prob`.
//! * **R2 — match parcial**: uma frase da lista embutida em texto maior só
//!   descarta com energia comprovadamente baixa.
//! * **R3 — repetição degenerada**: o output inteiro é uma unidade repetida
//!   (≥3 repetições de ≥2 palavras, ou ≥5 repetições de 1 palavra) → descarta
//!   sempre; loops de decode não são fala humana.
//! * **R0 — só tags**: output composto só de tags de efeito sonoro
//!   (`[Música]`, `(applause)`) → descarta sempre; nunca são ditado.
//!
//! O nível de segmento ([`filter_spans`]) usa a energia da janela `t0..t1` do
//! próprio segmento — um rabo fantasma sobre silêncio cai mesmo quando o
//! começo do áudio tinha fala. O nível de texto inteiro
//! ([`filter_phantom_text`]) roda no pós-processamento para todos os motores e
//! para o streaming (energia desconhecida = `None`).
//!
//! A lista embutida é documentada e fechada; "lista editável" (FR-003-13) fica
//! como seam de settings — os parâmetros `extra_blocklist` já aceitam frases
//! do usuário normalizadas pelo mesmo pipeline.

use log::info;
use once_cell::sync::Lazy;

/// RMS de pico-baixo: abaixo disto o trecho é tratado como silêncio/ruído de
/// fundo. Fala de ditado normal fica tipicamente ≥ 0.02; silêncio digital ~0.
pub(super) const LOW_ENERGY_RMS: f32 = 0.01;

/// Uma entrada da lista de bloqueio. `tag: true` marca pseudo-tokens de efeito
/// sonoro (`[Música]`, `(applause)`) — nunca fala real, descarte incondicional
/// quando compõem todo o output.
struct PhantomEntry {
    phrase: &'static str,
    tag: bool,
}

const fn phrase(phrase: &'static str) -> PhantomEntry {
    PhantomEntry { phrase, tag: false }
}

const fn tag(phrase: &'static str) -> PhantomEntry {
    PhantomEntry { phrase, tag: true }
}

/// Lista de bloqueio embutida (pt/en) — as formas documentadas; o matching é
/// feito sobre a forma normalizada (minúsculas, sem acento, sem pontuação).
///
/// Fonte: frases-fantasma conhecidas do whisper em silêncio/ruído (boilerplate
/// de vídeos legendados) + exemplos do FR-003-13.
const PHANTOM_ENTRIES: &[PhantomEntry] = &[
    // ---- português: boilerplate de legenda/chamada de canal ----
    phrase("Legendas pela comunidade Amara.org"),
    phrase("Legendas pela comunidade"),
    phrase("Legendas por"),
    phrase("Legenda por"),
    phrase("Tradução e legendas por"),
    phrase("Obrigado por assistir"),
    phrase("Obrigada por assistir"),
    phrase("Obrigado por ver"),
    phrase("Obrigado por ouvir"),
    phrase("Inscreva-se no canal"),
    phrase("Deixe seu like"),
    phrase("Deixe o seu like"),
    phrase("Deixe um like"),
    phrase("Ative o sininho"),
    phrase("Não se esqueça de se inscrever"),
    // ---- inglês ----
    phrase("Subtitles by the Amara.org community"),
    phrase("Subtitles by"),
    phrase("Captions by"),
    phrase("Transcribed by"),
    phrase("Translated by"),
    phrase("Thanks for watching"),
    phrase("Thank you for watching"),
    phrase("Thanks for listening"),
    phrase("Thank you for listening"),
    phrase("Thanks for watching!"),
    phrase("Please subscribe"),
    phrase("Like and subscribe"),
    phrase("Don't forget to subscribe"),
    phrase("See you in the next video"),
    // ---- tags de efeito sonoro — nunca são fala ditada ----
    tag("Música"),
    tag("Music"),
    tag("Aplausos"),
    tag("Applause"),
    tag("Risadas"),
    tag("Laughter"),
    tag("Silêncio"),
    tag("Silence"),
];

/// Entradas normalizadas, ordenadas por comprimento decrescente para que a
/// remoção prefira a frase mais longa ("subtitles by the amara org community"
/// antes de "subtitles by").
static BLOCKLIST: Lazy<Vec<(String, bool)>> = Lazy::new(|| {
    let mut entries: Vec<(String, bool)> = PHANTOM_ENTRIES
        .iter()
        .map(|entry| (normalize_for_match(entry.phrase), entry.tag))
        .filter(|(normalized, _)| !normalized.is_empty())
        .collect();
    entries.sort_by_key(|(normalized, _)| std::cmp::Reverse(normalized.len()));
    entries
});

/// Normalização para matching: minúsculas, sem acentos latinos comuns, só
/// alfanuméricos ASCII separados por espaço simples. Scripts não-latinos
/// (CJK, cirílico) viram vazio — o filtro fail-open não decide sobre eles.
fn normalize_for_match(text: &str) -> String {
    let mut normalized = String::with_capacity(text.len());
    let mut last_was_space = true;
    for ch in text.chars() {
        let folded = match ch.to_ascii_lowercase() {
            'á' | 'à' | 'â' | 'ã' | 'ä' | 'å' => 'a',
            'ç' => 'c',
            'é' | 'è' | 'ê' | 'ë' => 'e',
            'í' | 'ì' | 'î' | 'ï' => 'i',
            'ñ' => 'n',
            'ó' | 'ò' | 'ô' | 'õ' | 'ö' => 'o',
            'ú' | 'ù' | 'û' | 'ü' => 'u',
            'ý' | 'ÿ' => 'y',
            'ß' => 's',
            other => other,
        };
        if folded.is_ascii_alphanumeric() {
            normalized.push(folded);
            last_was_space = false;
        } else if !last_was_space {
            normalized.push(' ');
            last_was_space = true;
        }
    }
    normalized.trim().to_string()
}

/// `haystack` contém `needle` em fronteira de palavra (ambos já normalizados,
/// só `[a-z0-9 ]`). O padding com espaços torna o teste de borda trivial.
fn contains_phrase(haystack: &str, needle: &str) -> bool {
    format!(" {haystack} ").contains(&format!(" {needle} "))
}

/// Remove todas as ocorrências de `phrases` (em fronteira de palavra) e
/// devolve o que sobrou — o resíduo não-fantasma do texto.
fn residue_after_stripping(
    normalized: &str,
    phrases: &[(String, bool)],
    tags_only: bool,
) -> String {
    let mut padded = format!(" {normalized} ");
    for (phrase, is_tag) in phrases {
        if tags_only && !is_tag {
            continue;
        }
        let needle = format!(" {phrase} ");
        while padded.contains(&needle) {
            padded = padded.replacen(&needle, " ", 1);
        }
    }
    padded.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Saída degenerada: o texto inteiro é uma unidade repetida em sequência —
/// a assinatura de um loop de decode. ≥3 repetições quando a unidade tem ≥2
/// palavras; ≥5 quando é uma palavra só (abjurações curtas tipo "não não não"
/// são ênfase humana, não loop). Opera sobre o texto já normalizado.
fn is_degenerate_repetition(normalized: &str) -> bool {
    let words: Vec<&str> = normalized.split_whitespace().collect();
    let n = words.len();
    if n < 3 {
        return false;
    }
    // Unidade de 1 palavra repetida em tudo (ex.: "bye bye bye bye bye").
    if n >= 5 && words.iter().all(|w| *w == words[0]) {
        return true;
    }
    // Unidade multi-palavra cobrindo o texto inteiro: words == unit × k.
    for unit in 2..=n / 3 {
        if !n.is_multiple_of(unit) {
            continue;
        }
        let reps = n / unit;
        if reps >= 3 && (0..reps).all(|r| words[r * unit..(r + 1) * unit] == words[..unit]) {
            return true;
        }
    }
    false
}

/// Decide se um texto (output inteiro ou um segmento) é alucinação a
/// descartar. `energy` é o RMS das amostras que geraram o texto; `None` =
/// sem evidência de energia (caminho de streaming), caso em que R1 descarta
/// e R2 não. Energia não-finita (NaN/∞) é tratada como desconhecida.
pub(super) fn is_phantom_output(
    text: &str,
    energy: Option<f32>,
    extra_blocklist: &[String],
) -> bool {
    let energy = energy.filter(|rms| rms.is_finite());
    let normalized = normalize_for_match(text);
    if normalized.is_empty() {
        return false;
    }

    let mut blocklist: Vec<(String, bool)> = BLOCKLIST.clone();
    blocklist.extend(
        extra_blocklist
            .iter()
            .map(|entry| (normalize_for_match(entry), false))
            .filter(|(normalized, _)| !normalized.is_empty()),
    );
    blocklist.sort_by_key(|(normalized, _)| std::cmp::Reverse(normalized.len()));

    // R0: output composto só de tags de efeito sonoro — nunca ditado.
    if residue_after_stripping(&normalized, &blocklist, true).is_empty() {
        return true;
    }

    // R1: output composto só de frases da lista de bloqueio.
    if residue_after_stripping(&normalized, &blocklist, false).is_empty() {
        return energy.is_none_or(|rms| rms < LOW_ENERGY_RMS);
    }

    // R2: frase de ≥2 palavras embutida em texto maior — só descarta com
    // energia comprovadamente baixa (tags de 1 palavra ficam de fora para
    // não derrubar "a música foi boa" dito baixinho).
    let embedded_phrase = blocklist.iter().any(|(phrase, _)| {
        phrase.split_whitespace().nth(1).is_some() && contains_phrase(&normalized, phrase)
    });
    if embedded_phrase {
        return matches!(energy, Some(rms) if rms < LOW_ENERGY_RMS);
    }

    // R3: loop de decode.
    is_degenerate_repetition(&normalized)
}

/// Filtro de texto inteiro para o pós-processamento (FR-003-13). Retorna
/// string vazia quando o output é alucinação — o chamador trata como
/// "nada ouvido" e nada é colado.
pub(super) fn filter_phantom_text(text: String, energy: Option<f32>) -> String {
    if is_phantom_output(&text, energy, &[]) {
        info!(
            "Hallucination filter dropped output (energy={:?}): {:?}",
            energy,
            crate::utils::redact_text(&text)
        );
        String::new()
    } else {
        text
    }
}

/// Um segmento de engine com janela de áudio (ms) — versão desacoplada do
/// `transcribe_cpp::Segment` para o filtro ser testável sem o motor.
pub(super) struct PhantomSpan {
    pub t0_ms: i64,
    pub t1_ms: i64,
    pub text: String,
}

impl PhantomSpan {
    pub(super) fn new(t0_ms: i64, t1_ms: i64, text: String) -> Self {
        Self { t0_ms, t1_ms, text }
    }
}

/// RMS das amostras; `None` quando não há amostras.
pub(super) fn overall_rms(samples: &[f32]) -> Option<f32> {
    if samples.is_empty() {
        return None;
    }
    let mean_square = samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32;
    Some(mean_square.sqrt())
}

/// RMS da janela `[t0_ms, t1_ms)` das amostras. `None` quando a janela é
/// inválida ou vazia — o chamador trata como energia desconhecida.
fn window_rms(samples: &[f32], sample_rate: u32, t0_ms: i64, t1_ms: i64) -> Option<f32> {
    let start = (t0_ms.max(0) as u64 * sample_rate as u64 / 1000) as usize;
    let end = ((t1_ms.max(0) as u64 * sample_rate as u64 / 1000) as usize).min(samples.len());
    if end <= start || start >= samples.len() {
        return None;
    }
    overall_rms(&samples[start..end])
}

/// Filtro por segmento para motores que devolvem timestamps (transcribe-cpp):
/// cada segmento é testado contra a lista de bloqueio com a energia da sua
/// própria janela de áudio — um rabo fantasma sobre silêncio cai mesmo depois
/// de fala real. Texto mantido é concatenado na ordem original.
pub(super) fn filter_spans(spans: &[PhantomSpan], samples: &[f32], sample_rate: u32) -> String {
    let mut kept = String::new();
    for span in spans {
        let energy = window_rms(samples, sample_rate, span.t0_ms, span.t1_ms);
        if is_phantom_output(&span.text, energy, &[]) {
            info!(
                "Hallucination filter dropped span {}..{}ms (energy={:?}): {:?}",
                span.t0_ms,
                span.t1_ms,
                energy,
                crate::utils::redact_text(&span.text)
            );
            continue;
        }
        kept.push_str(&span.text);
    }
    kept
}

#[cfg(test)]
mod tests {
    use super::*;

    const SILENT: Option<f32> = Some(0.0);
    const SPEECH: Option<f32> = Some(0.2);

    #[test]
    fn normalization_folds_case_accents_and_punctuation() {
        assert_eq!(
            normalize_for_match("  Obrigado por Assistir!! "),
            "obrigado por assistir"
        );
        assert_eq!(normalize_for_match("[Música]"), "musica");
        assert_eq!(normalize_for_match("(Applause)"), "applause");
        assert_eq!(normalize_for_match("Não, não sei..."), "nao nao sei");
    }

    #[test]
    fn spec_examples_drop_on_silence() {
        // AC-003-05 / FR-003-13: frases-fantasma documentadas sobre áudio
        // sem fala não podem virar texto colado.
        for phantom in [
            "Legendas pela comunidade Amara.org",
            "Obrigado por assistir",
            "Inscreva-se no canal",
            "Thanks for watching",
            "[Música]",
        ] {
            assert!(
                is_phantom_output(phantom, SILENT, &[]),
                "expected '{phantom}' to be filtered on silence"
            );
        }
    }

    #[test]
    fn boilerplate_kept_when_energy_is_real_speech() {
        // "e/ou energia baixa" da spec: dito com energia de fala, pode ser um
        // roteiro de vídeo ditado de verdade.
        assert!(!is_phantom_output("Obrigado por assistir", SPEECH, &[]));
        assert!(!is_phantom_output("Thanks for watching", SPEECH, &[]));
    }

    #[test]
    fn boilerplate_drops_when_energy_is_unknown() {
        // Caminho de streaming: sem evidência de energia, match exato cai.
        assert!(is_phantom_output("Obrigado por assistir.", None, &[]));
    }

    #[test]
    fn sound_tags_drop_unconditionally() {
        // Ninguém "dita" uma tag de efeito sonoro.
        assert!(is_phantom_output("[Música]", SPEECH, &[]));
        assert!(is_phantom_output("(applause) (laughter)", SPEECH, &[]));
        assert!(is_phantom_output("[Music]", SILENT, &[]));
    }

    #[test]
    fn composed_of_only_blocklisted_phrases_drops() {
        assert!(is_phantom_output(
            "Thanks for watching. Thanks for watching.",
            Some(f32::NAN), // energia quebrada = desconhecida → descarta
            &[]
        ));
        assert!(is_phantom_output(
            "[Música] Obrigado por assistir!",
            SILENT,
            &[]
        ));
    }

    #[test]
    fn embedded_phrase_needs_low_energy() {
        let text = "obrigado por assistir ao vídeo que gravei ontem";
        assert!(is_phantom_output(text, SILENT, &[]));
        assert!(!is_phantom_output(text, SPEECH, &[]));
        // Sem evidência de energia, match parcial conserva o texto.
        assert!(!is_phantom_output(text, None, &[]));
    }

    #[test]
    fn single_word_tag_inside_real_text_never_triggers_partial_match() {
        assert!(!is_phantom_output("a música foi boa demais", SILENT, &[]));
        assert!(!is_phantom_output("escolhi music para a cena", SILENT, &[]));
    }

    #[test]
    fn real_dictation_survives() {
        for text in [
            "preciso revisar o relatório até sexta",
            "the quick brown fox jumps over the lazy dog",
            "obrigado, até amanhã então",
            "por favor me manda o arquivo",
        ] {
            assert!(
                !is_phantom_output(text, SILENT, &[]),
                "real dictation '{text}' must survive"
            );
            assert!(!is_phantom_output(text, SPEECH, &[]));
            assert!(!is_phantom_output(text, None, &[]));
        }
    }

    #[test]
    fn degenerate_repetition_drops() {
        assert!(is_phantom_output(
            "Thank you. Thank you. Thank you.",
            SPEECH,
            &[]
        ));
        assert!(is_phantom_output(
            "eu não sei eu não sei eu não sei",
            SPEECH,
            &[]
        ));
        assert!(is_phantom_output("bye bye bye bye bye", SPEECH, &[]));
    }

    #[test]
    fn human_emphasis_repetition_survives() {
        // "não não não" é ênfase humana; unidade de 1 palavra com <5 reps
        // não é loop de decode. (O normalizador de stutter já colapsa a
        // repetição na saída.)
        assert!(!is_phantom_output("não não não", SPEECH, &[]));
        assert!(!is_phantom_output(
            "muito obrigado muito obrigado",
            SPEECH,
            &[]
        ));
    }

    #[test]
    fn non_latin_text_fails_open() {
        assert!(!is_phantom_output("你好世界", SILENT, &[]));
        assert!(!is_phantom_output("こんにちは", SILENT, &[]));
    }

    #[test]
    fn extra_blocklist_entries_apply() {
        let extra = vec!["frase fantasma customizada".to_string()];
        assert!(is_phantom_output(
            "Frase Fantasma Customizada!",
            SILENT,
            &extra
        ));
        assert!(!is_phantom_output(
            "frase fantasma customizada",
            SILENT,
            &[]
        ));
    }

    #[test]
    fn window_rms_measures_only_the_span() {
        // 2 s de "fala" forte + 3 s de silêncio a 16 kHz.
        let mut samples = vec![0.5f32; 32_000];
        samples.extend(std::iter::repeat_n(0.0, 48_000));

        let speech = window_rms(&samples, 16_000, 0, 2_000).unwrap();
        let silence = window_rms(&samples, 16_000, 2_000, 5_000).unwrap();
        assert!(speech >= LOW_ENERGY_RMS);
        assert_eq!(silence, 0.0);
        // Janela fora do buffer é "energia desconhecida".
        assert_eq!(window_rms(&samples, 16_000, 9_000, 12_000), None);
        assert_eq!(window_rms(&[], 16_000, 0, 1_000), None);
    }

    #[test]
    fn filter_spans_drops_only_the_phantom_tail() {
        let mut samples = vec![0.4f32; 16_000]; // 1 s de fala
        samples.extend(std::iter::repeat_n(0.0, 32_000)); // 2 s de silêncio

        let spans = vec![
            PhantomSpan::new(0, 1_000, " oi tudo bem".to_string()),
            PhantomSpan::new(1_000, 3_000, " Obrigado por assistir.".to_string()),
        ];

        assert_eq!(
            filter_spans(&spans, &samples, 16_000),
            " oi tudo bem".to_string()
        );
    }

    #[test]
    fn filter_spans_keeps_boilerplate_spoken_with_energy() {
        let samples = vec![0.4f32; 48_000];
        let spans = vec![PhantomSpan::new(
            0,
            3_000,
            " Obrigado por assistir".to_string(),
        )];
        assert_eq!(
            filter_spans(&spans, &samples, 16_000),
            " Obrigado por assistir".to_string()
        );
    }

    #[test]
    fn all_phantom_spans_filter_to_empty() {
        let samples = vec![0.0f32; 48_000];
        let spans = vec![
            PhantomSpan::new(0, 1_500, " Thanks for watching.".to_string()),
            PhantomSpan::new(1_500, 3_000, " [Música]".to_string()),
        ];
        assert_eq!(filter_spans(&spans, &samples, 16_000), "");
    }

    #[test]
    fn filter_phantom_text_returns_empty_on_phantom() {
        assert_eq!(
            filter_phantom_text("Obrigado por assistir.".to_string(), SILENT),
            ""
        );
        assert_eq!(
            filter_phantom_text("texto de verdade".to_string(), SILENT),
            "texto de verdade"
        );
    }
}
