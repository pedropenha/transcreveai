//! Seleção de provedor STT por uso (ditado / reunião / fallback) — FR-003-03.
//!
//! Na v1 ainda não existem linhas na tabela `providers` (data-model §2): os
//! provedores configuráveis são os modelos locais instalados, endereçados pelo
//! pseudo-id `local_model:<model_id>` nos campos `dictation_provider_id` /
//! `meeting_provider_id` / `fallback_provider_id` das settings. O prefixo
//! mantém o valor inequívoco quando a v1.1 trouxer providers de nuvem: ids sem
//! o prefixo são chaves da tabela `providers`.
//!
//! Ditado é o único uso sem id próprio na v1: trocar o modelo de ditado é
//! trocar `selected_model` (o motor real é carregado por
//! `commands::models::switch_active_model`), então `dictation_provider_id`
//! fica `None` e a resolução cai no `selected_model`.

use crate::settings::AppSettings;

/// Prefixo dos pseudo-ids de provedor local na v1 (`local_model:<model_id>`).
pub const LOCAL_MODEL_PROVIDER_PREFIX: &str = "local_model:";

/// Id de provedor que endereça um modelo local instalado.
pub fn local_model_provider_id(model_id: &str) -> String {
    format!("{LOCAL_MODEL_PROVIDER_PREFIX}{model_id}")
}

/// Se `provider_id` endereça um modelo local, devolve o id do modelo.
/// Ids sem o prefixo são chaves da tabela `providers` (v1.1+).
pub fn local_model_id(provider_id: &str) -> Option<&str> {
    provider_id
        .strip_prefix(LOCAL_MODEL_PROVIDER_PREFIX)
        .filter(|id| !id.is_empty())
}

/// Valor persistível numa coluna `*_provider_id REFERENCES providers(id)`:
/// apenas ids de linhas reais da tabela `providers`. Pseudo-ids
/// `local_model:*` não são chaves dessa tabela — na v1 ela está vazia e a FK
/// falharia — então viram `NULL` (o modelo efetivo continua resolvível via
/// `effective_*_model_id`).
pub fn provider_row_id(provider_id: Option<&str>) -> Option<String> {
    provider_id
        .filter(|id| local_model_id(id).is_none())
        .map(str::to_string)
}

/// O campo `* _provider_id` referencia o modelo local `model_id`?
pub fn provider_is_local_model(provider_id: Option<&str>, model_id: &str) -> bool {
    provider_id
        .and_then(local_model_id)
        .is_some_and(|id| id == model_id)
}

/// Modelo que o ditado usa de fato: `dictation_provider_id` quando aponta para
/// um modelo local, senão `selected_model` (vazio = nenhum).
pub fn effective_dictation_model_id(settings: &AppSettings) -> Option<String> {
    settings
        .dictation_provider_id
        .as_deref()
        .and_then(local_model_id)
        .map(str::to_string)
        .or_else(|| (!settings.selected_model.is_empty()).then(|| settings.selected_model.clone()))
}

/// Modelo que a reunião usa de fato: `meeting_provider_id` quando aponta para
/// um modelo local; `None` herda a seleção de ditado (FR-003-03: a seleção de
/// reunião é separada, mas opcional — "igual ao ditado" é o padrão).
pub fn effective_meeting_model_id(settings: &AppSettings) -> Option<String> {
    settings
        .meeting_provider_id
        .as_deref()
        .and_then(local_model_id)
        .map(str::to_string)
        .or_else(|| effective_dictation_model_id(settings))
}

/// Modelo de fallback configurado, se apontar para um modelo local.
/// (v1: persistido para quando o orquestrador ganhar fallback local/nuvem.)
pub fn effective_fallback_model_id(settings: &AppSettings) -> Option<String> {
    settings
        .fallback_provider_id
        .as_deref()
        .and_then(local_model_id)
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::get_default_settings;

    fn settings() -> AppSettings {
        get_default_settings()
    }

    #[test]
    fn local_model_provider_id_roundtrip() {
        let pid = local_model_provider_id("whisper-small");
        assert_eq!(pid, "local_model:whisper-small");
        assert_eq!(local_model_id(&pid), Some("whisper-small"));
    }

    #[test]
    fn local_model_id_rejects_other_provider_ids() {
        // uuids de linhas da tabela `providers` (v1.1+) não decodificam.
        assert_eq!(local_model_id("9f0c…-uuid"), None);
        assert_eq!(local_model_id("openai"), None);
        assert_eq!(local_model_id("local_model:"), None);
    }

    #[test]
    fn provider_row_id_keeps_only_real_provider_keys() {
        // Pseudo-ids de modelo local não são chaves de `providers` — gravá-los
        // numa coluna REFERENCES providers(id) quebraria a FK (a tabela está
        // vazia na v1).
        assert_eq!(provider_row_id(Some("local_model:whisper-turbo")), None);
        assert_eq!(provider_row_id(None), None);
        assert_eq!(
            provider_row_id(Some("9f0c…-uuid")),
            Some("9f0c…-uuid".to_string())
        );
    }

    #[test]
    fn dictation_defaults_to_selected_model() {
        let mut s = settings();
        s.selected_model = "whisper-small".to_string();
        assert_eq!(
            effective_dictation_model_id(&s).as_deref(),
            Some("whisper-small")
        );
        // Um provider local explícito prevalece sobre selected_model.
        s.dictation_provider_id = Some(local_model_provider_id("whisper-base"));
        assert_eq!(
            effective_dictation_model_id(&s).as_deref(),
            Some("whisper-base")
        );
        // Ids desconhecidos (providers de nuvem) não resolvem para modelo local
        // — a resolução cai em selected_model, que é o caminho da v1.
        s.dictation_provider_id = Some("cloud-uuid".to_string());
        assert_eq!(
            effective_dictation_model_id(&s).as_deref(),
            Some("whisper-small")
        );
    }

    #[test]
    fn meeting_inherits_dictation_when_unset() {
        let mut s = settings();
        s.selected_model = "whisper-small".to_string();
        assert_eq!(
            effective_meeting_model_id(&s).as_deref(),
            Some("whisper-small")
        );

        s.meeting_provider_id = Some(local_model_provider_id("whisper-turbo"));
        assert_eq!(
            effective_meeting_model_id(&s).as_deref(),
            Some("whisper-turbo")
        );
    }

    #[test]
    fn fallback_only_resolves_local_model_ids() {
        let mut s = settings();
        assert_eq!(effective_fallback_model_id(&s), None);
        s.fallback_provider_id = Some(local_model_provider_id("whisper-tiny"));
        assert_eq!(
            effective_fallback_model_id(&s).as_deref(),
            Some("whisper-tiny")
        );
        s.fallback_provider_id = Some("cloud-uuid".to_string());
        assert_eq!(effective_fallback_model_id(&s), None);
    }

    #[test]
    fn provider_is_local_model_matches_only_exact_id() {
        let pid = local_model_provider_id("m1");
        assert!(provider_is_local_model(Some(&pid), "m1"));
        assert!(!provider_is_local_model(Some(&pid), "m2"));
        assert!(!provider_is_local_model(Some("uuid"), "m1"));
        assert!(!provider_is_local_model(None, "m1"));
    }

    #[test]
    fn empty_selected_model_resolves_to_none() {
        let s = settings();
        assert_eq!(effective_dictation_model_id(&s), None);
        assert_eq!(effective_meeting_model_id(&s), None);
    }
}
