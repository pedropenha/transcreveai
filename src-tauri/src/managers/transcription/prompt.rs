//! Montagem do `initial_prompt` do whisper a partir das dicas de vocabulário
//! (`SttOptions::vocabulary_hints`, alimentado por `settings.custom_words`) —
//! FR-003-12.
//!
//! O whisper aceita um prompt de contexto limitado pela janela de decodificação
//! (~224 tokens de prefixo na prática); a spec reserva ~200. Como não existe
//! tokenizer local, o orçamento é estimado de forma determinística (≈4
//! caracteres latinos por token) e os termos entram na ordem em que foram
//! cadastrados — a ordem da lista É a prioridade ("mais usados" primeiro cabe
//! ao usuário na tela do dicionário; ranking automático por uso é pós-v1).

/// Orçamento de tokens do `initial_prompt` (FR-003-12: ~200 tokens).
pub(super) const INITIAL_PROMPT_TOKEN_BUDGET: usize = 200;

/// Estimativa de tokens whisper para um texto: ~4 chars/token em texto latino
/// (whisper BPE), mínimo 1 por termo não-vazio.
fn estimated_tokens(text: &str) -> usize {
    (text.chars().count() / 4).max(1)
}

/// Constrói o `initial_prompt` juntando os termos com `", "` até o orçamento
/// de tokens. Termos são adicionados na ordem recebida (prioridade = ordem da
/// lista), sem duplicatas — a primeira ocorrência (case-insensitive) vence e
/// repetições não gastam orçamento. O primeiro termo que estouraria o
/// orçamento encerra a montagem, e um termo sozinho maior que o orçamento é
/// descartado por inteiro — um prompt truncado no meio de uma palavra enviesa
/// o decoder para texto quebrado.
///
/// `None` quando nenhum termo cabe (lista vazia/só espaços).
pub(super) fn build_initial_prompt(hints: &[String]) -> Option<String> {
    let mut prompt = String::new();
    let mut used = 0usize;
    let mut seen = std::collections::HashSet::new();

    for hint in hints {
        let hint = hint.trim();
        if hint.is_empty() || !seen.insert(hint.to_lowercase()) {
            continue;
        }
        // Cada termo além do primeiro paga o separador ", " (~1 token).
        let separator = usize::from(!prompt.is_empty());
        let cost = estimated_tokens(hint) + separator;
        if used + cost > INITIAL_PROMPT_TOKEN_BUDGET {
            break;
        }
        if separator == 1 {
            prompt.push_str(", ");
        }
        prompt.push_str(hint);
        used += cost;
    }

    if prompt.is_empty() {
        None
    } else {
        Some(prompt)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hints(words: &[&str]) -> Vec<String> {
        words.iter().map(|w| (*w).to_string()).collect()
    }

    #[test]
    fn empty_hints_produce_no_prompt() {
        assert_eq!(build_initial_prompt(&[]), None);
        assert_eq!(build_initial_prompt(&hints(&["", "   "])), None);
    }

    #[test]
    fn hints_join_with_comma_separator() {
        assert_eq!(
            build_initial_prompt(&hints(&["Kubernetes", "Transcreve.ai"])),
            Some("Kubernetes, Transcreve.ai".to_string())
        );
    }

    #[test]
    fn hints_are_trimmed() {
        assert_eq!(
            build_initial_prompt(&hints(&["  Kubernetes  "])),
            Some("Kubernetes".to_string())
        );
    }

    #[test]
    fn order_is_preserved() {
        assert_eq!(
            build_initial_prompt(&hints(&["Zebra", "Alpha"])),
            Some("Zebra, Alpha".to_string())
        );
    }

    #[test]
    fn duplicates_are_dropped_case_insensitively() {
        // A primeira ocorrência vence; repetidos não gastam orçamento.
        assert_eq!(
            build_initial_prompt(&hints(&["Kubernetes", "kubernetes", "KUBERNETES", "KEDA"])),
            Some("Kubernetes, KEDA".to_string())
        );
        assert_eq!(
            build_initial_prompt(&hints(&["  KubE  ", "kube"])),
            Some("KubE".to_string())
        );
    }

    #[test]
    fn budget_stops_before_the_term_that_would_overflow() {
        // 200 tokens ≈ ~800 chars de termos + separadores. 40 termos de 20
        // chars custam ~6 tokens cada → ~33 cabem, o 34º estoura.
        let many: Vec<String> = (0..40).map(|i| format!("termocom{i:013}")).collect();
        let prompt = build_initial_prompt(&many).unwrap();

        // Todos os termos incluídos aparecem inteiros; o primeiro excluído
        // não pode aparecer nem parcialmente.
        assert!(prompt.contains("termocom"));
        let included = prompt.split(", ").count();
        assert!(included < 40);
        assert!(included >= 30);
        assert!(!prompt.ends_with(','));
    }

    #[test]
    fn a_single_oversized_term_yields_no_prompt() {
        let huge = "x".repeat(INITIAL_PROMPT_TOKEN_BUDGET * 4 + 8);
        assert_eq!(build_initial_prompt(&[huge]), None);
    }
}
