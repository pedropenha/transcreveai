use super::*;

/// Exercise the complete cleanup sequence with an explicitly selected
/// language. Individual tests below predate the split between filler
/// removal and non-filler normalization.
fn filter_transcription_output(
    text: &str,
    language: &str,
    custom_filler_words: &Option<Vec<String>>,
) -> String {
    let language = OutputLanguageEvidence::UserSelected(language.to_string());
    let filtered = remove_filler_words(text, &language, custom_filler_words, true);
    normalize_transcription_output(&filtered)
}

#[test]
fn test_apply_custom_words_exact_match() {
    let text = "hello world";
    let custom_words = vec!["Hello".to_string(), "World".to_string()];
    let result = apply_custom_words(text, &custom_words, 0.5);
    assert_eq!(result, "Hello World");
}

#[test]
fn test_apply_custom_words_fuzzy_match() {
    let text = "helo wrold";
    let custom_words = vec!["hello".to_string(), "world".to_string()];
    let result = apply_custom_words(text, &custom_words, 0.5);
    assert_eq!(result, "hello world");
}

#[test]
fn test_preserve_case_pattern() {
    assert_eq!(preserve_case_pattern("HELLO", "world"), "WORLD");
    assert_eq!(preserve_case_pattern("Hello", "world"), "World");
    assert_eq!(preserve_case_pattern("hello", "WORLD"), "WORLD");
}

#[test]
fn test_extract_punctuation() {
    assert_eq!(extract_punctuation("hello"), ("", ""));
    assert_eq!(extract_punctuation("!hello?"), ("!", "?"));
    assert_eq!(extract_punctuation("...hello..."), ("...", "..."));
}

#[test]
fn test_extract_punctuation_uses_unicode_boundaries() {
    assert_eq!(extract_punctuation("你好。"), ("", "。"));
    assert_eq!(extract_punctuation("「你好」"), ("「", "」"));
    assert_eq!(extract_punctuation("你好！"), ("", "！"));
}

#[test]
fn test_empty_custom_words() {
    let text = "hello world";
    let custom_words = vec![];
    let result = apply_custom_words(text, &custom_words, 0.5);
    assert_eq!(result, "hello world");
}

#[test]
fn test_filter_filler_words() {
    let text = "So uhm I was thinking uh about this";
    let result = filter_transcription_output(text, "en", &None);
    assert_eq!(result, "So I was thinking about this");
}

#[test]
fn test_filter_filler_words_case_insensitive() {
    let text = "UHM this is UH a test";
    let result = filter_transcription_output(text, "en", &None);
    assert_eq!(result, "This is a test");
}

#[test]
fn test_filter_filler_words_with_punctuation() {
    let text = "Well, uhm, I think, uh. that's right";
    let result = filter_transcription_output(text, "en", &None);
    assert_eq!(result, "Well, I think, that's right");
}

#[test]
fn test_filter_cleans_whitespace() {
    let text = "Hello    world   test";
    let result = filter_transcription_output(text, "en", &None);
    assert_eq!(result, "Hello world test");
}

#[test]
fn test_filter_trims() {
    let text = "  Hello world  ";
    let result = filter_transcription_output(text, "en", &None);
    assert_eq!(result, "Hello world");
}

#[test]
fn test_filter_combined() {
    let text = "  Uhm, so I was, uh, thinking about this  ";
    let result = filter_transcription_output(text, "en", &None);
    assert_eq!(result, "So I was, thinking about this");
}

#[test]
fn test_filter_leading_filler_keeps_sentence_capital() {
    let result = filter_transcription_output("Um, so I think we should ship it.", "en", &None);
    assert_eq!(result, "So I think we should ship it.");

    let result = filter_transcription_output("That works. Um, let me check.", "en", &None);
    assert_eq!(result, "That works. Let me check.");

    // Mid-sentence there is no capital to hand over.
    let result = filter_transcription_output("He said, Um, not today.", "en", &None);
    assert_eq!(result, "He said, not today.");
}

#[test]
fn test_filter_preserves_valid_text() {
    let text = "This is a completely normal sentence.";
    let result = filter_transcription_output(text, "en", &None);
    assert_eq!(result, "This is a completely normal sentence.");
}

#[test]
fn test_filter_stutter_collapse() {
    let text = "w wh wh wh wh wh wh wh wh wh why";
    let result = filter_transcription_output(text, "en", &None);
    assert_eq!(result, "w wh why");
}

#[test]
fn test_filter_stutter_short_words() {
    let text = "I I I I think so so so so";
    let result = filter_transcription_output(text, "en", &None);
    assert_eq!(result, "I think so");
}

#[test]
fn test_filter_stutter_longer_words() {
    let text = "Check data doc doc doc doc documentation.";
    let result = filter_transcription_output(text, "en", &None);
    assert_eq!(result, "Check data doc documentation.");
}

#[test]
fn test_filter_stutter_mixed_case() {
    let text = "No NO no NO no";
    let result = filter_transcription_output(text, "en", &None);
    assert_eq!(result, "No");
}

#[test]
fn test_filter_stutter_preserves_two_repetitions() {
    let text = "no no is fine";
    let result = filter_transcription_output(text, "en", &None);
    assert_eq!(result, "no no is fine");
}

#[test]
fn test_filter_english_removes_um() {
    let text = "um I think um this is good";
    let result = filter_transcription_output(text, "en", &None);
    assert_eq!(result, "I think this is good");
}

#[test]
fn test_filter_portuguese_preserves_um() {
    // "um" means "a/an" in Portuguese
    let text = "um gato bonito";
    let result = filter_transcription_output(text, "pt", &None);
    assert_eq!(result, "um gato bonito");
}

#[test]
fn test_filter_spanish_preserves_ha() {
    // "ha" means "has" in Spanish
    let text = "ha sido un buen día";
    let result = filter_transcription_output(text, "es", &None);
    assert_eq!(result, "ha sido un buen día");
}

#[test]
fn test_filter_language_code_with_region() {
    // "pt-BR" should normalize to "pt"
    let text = "um gato bonito";
    let result = filter_transcription_output(text, "pt-BR", &None);
    assert_eq!(result, "um gato bonito");
}

#[test]
fn test_filter_custom_filler_words_override() {
    let custom = Some(vec!["okay".to_string(), "right".to_string()]);
    let text = "okay so I think right this works";
    let result = filter_transcription_output(text, "en", &custom);
    assert_eq!(result, "so I think this works");
}

#[test]
fn test_filter_custom_filler_words_empty_disables() {
    let custom = Some(vec![]);
    let text = "So uhm I was thinking uh about this";
    let result = filter_transcription_output(text, "en", &custom);
    // No filler words removed since custom list is empty
    assert_eq!(result, "So uhm I was thinking uh about this");
}

#[test]
fn test_filter_unknown_language_still_removes_universal_fillers() {
    let text = "uh I think uhm this works";
    let result = filter_transcription_output(text, "xx", &None);
    assert_eq!(result, "I think this works");
}

#[test]
fn test_filter_unknown_language_does_not_remove_um() {
    let text = "um I think this works";
    let result = filter_transcription_output(text, "xx", &None);
    assert_eq!(result, "um I think this works");
}

#[test]
fn test_filter_unknown_evidence_removes_universal_keeps_gated() {
    let filtered = remove_filler_words(
        "uhh bueno hmm creo que um ha llegado",
        &OutputLanguageEvidence::Unknown,
        &None,
        true,
    );
    assert_eq!(
        normalize_transcription_output(&filtered),
        "bueno creo que um ha llegado"
    );

    let cyrillic = remove_filler_words(
        "хм я думаю ммм это работает",
        &OutputLanguageEvidence::Unknown,
        &None,
        true,
    );
    assert_eq!(
        normalize_transcription_output(&cyrillic),
        "я думаю это работает"
    );
}

#[test]
fn test_filter_german_gated_fillers_require_evidence() {
    let text = "äh ich glaube ähm das passt";

    let unknown = remove_filler_words(text, &OutputLanguageEvidence::Unknown, &None, true);
    assert_eq!(normalize_transcription_output(&unknown), text);

    let result = filter_transcription_output(text, "de", &None);
    assert_eq!(result, "ich glaube das passt");
}

#[test]
fn test_filter_preserves_millimetre_unit() {
    // "mm" was removed from the filler lists because it eats units.
    let text = "the screw is 5 mm long";
    let result = filter_transcription_output(text, "en", &None);
    assert_eq!(result, "the screw is 5 mm long");
}

#[test]
fn test_filter_detected_evidence_unlocks_gated_fillers() {
    let model = remove_filler_words(
        "um I think this works",
        &OutputLanguageEvidence::ModelDetected("en".to_string()),
        &None,
        true,
    );
    assert_eq!(normalize_transcription_output(&model), "I think this works");

    let text = remove_filler_words(
        "euh je pense que ça marche",
        &OutputLanguageEvidence::TextDetected("fr".to_string()),
        &None,
        true,
    );
    assert_eq!(
        normalize_transcription_output(&text),
        "je pense que ça marche"
    );
}

#[test]
fn test_filter_master_toggle_disables_custom_and_builtin_removal() {
    let text = "um customword I think";
    let language = OutputLanguageEvidence::UserSelected("en".to_string());
    let custom = Some(vec!["customword".to_string()]);

    let result = remove_filler_words(text, &language, &custom, false);

    assert_eq!(result, text);
}

#[test]
fn test_filter_custom_words_apply_without_language_evidence() {
    let custom = Some(vec!["customword".to_string()]);
    let text = "customword should be removed but um should remain";

    let filtered = remove_filler_words(text, &OutputLanguageEvidence::Unknown, &custom, true);
    let result = normalize_transcription_output(&filtered);

    assert_eq!(result, "should be removed but um should remain");
}

#[test]
fn test_apply_custom_words_ngram_two_words() {
    let text = "il cui nome è Charge B, che permette";
    let custom_words = vec!["ChargeBee".to_string()];
    let result = apply_custom_words(text, &custom_words, 0.5);
    assert!(result.contains("ChargeBee,"), "unexpected result: {result}");
    assert!(!result.contains("Charge B"));
}

#[test]
fn test_apply_custom_words_ngram_three_words() {
    let text = "use Chat G P T for this";
    let custom_words = vec!["ChatGPT".to_string()];
    let result = apply_custom_words(text, &custom_words, 0.5);
    assert!(result.contains("ChatGPT"));
}

#[test]
fn test_apply_custom_words_prefers_longer_ngram() {
    let text = "Open AI GPT model";
    let custom_words = vec!["OpenAI".to_string(), "GPT".to_string()];
    let result = apply_custom_words(text, &custom_words, 0.5);
    assert_eq!(result, "OpenAI GPT model");
}

#[test]
fn test_apply_custom_words_ngram_preserves_case() {
    let text = "CHARGE B is great";
    let custom_words = vec!["ChargeBee".to_string()];
    let result = apply_custom_words(text, &custom_words, 0.5);
    assert!(result.contains("CHARGEBEE"));
}

#[test]
fn test_apply_custom_words_ngram_with_spaces_in_custom() {
    // Custom word with space should also match against split words
    let text = "using Mac Book Pro";
    let custom_words = vec!["MacBook Pro".to_string()];
    let result = apply_custom_words(text, &custom_words, 0.5);
    assert_eq!(result, "using MacBook Pro");
}

#[test]
fn test_apply_custom_words_trailing_number_not_doubled() {
    // Verify that trailing non-alpha chars (like numbers) aren't double-counted
    // between build_ngram stripping them and extract_punctuation capturing them
    let text = "use GPT4 for this";
    let custom_words = vec!["GPT-4".to_string()];
    let result = apply_custom_words(text, &custom_words, 0.5);
    // Should NOT produce "GPT-44" (double-counting the trailing 4)
    assert!(
        !result.contains("GPT-44"),
        "got double-counted result: {}",
        result
    );
}

#[test]
fn test_apply_custom_words_matches_ampersand_word() {
    let text = "send it to RD for review";
    let custom_words = vec!["R&D".to_string()];
    let result = apply_custom_words(text, &custom_words, 0.18);
    assert_eq!(result, "send it to R&D for review");
}

#[test]
fn test_apply_custom_words_matches_spoken_ampersand_word() {
    let text = "send it to R and D for review";
    let custom_words = vec!["R&D".to_string()];
    let result = apply_custom_words(text, &custom_words, 0.18);
    assert_eq!(result, "send it to R&D for review");
}

#[test]
fn test_apply_custom_words_preserves_ampersand_word() {
    let text = "send it to R&D for review";
    let custom_words = vec!["R&D".to_string()];
    let result = apply_custom_words(text, &custom_words, 0.18);
    assert_eq!(result, "send it to R&D for review");
}

#[test]
fn test_apply_custom_words_handles_unicode_punctuation() {
    let text = "「Handee。」";
    let custom_words = vec!["Handy".to_string()];
    let result = apply_custom_words(text, &custom_words, 0.5);
    assert_eq!(result, "「Handy。」");
}

#[test]
fn test_apply_custom_words_skips_cjk_fuzzy_matching() {
    let text = "你好。";
    let custom_words = vec!["你号".to_string()];
    let result = apply_custom_words(text, &custom_words, 1.0);
    assert_eq!(result, text);
}
