//! Pipeline de texto determinístico (F004).
//!
//! Transforma o texto cru da transcrição em texto pronto para inserção:
//!
//! ```text
//! raw ──► 1 normalizar ──► 2 comandos de voz ──► 4 dicionário (vocab)
//!     ──► 5 limpeza `light` ──► final
//! ```
//!
//! As etapas 3 (snippets), 6 (estilos por app) e a limpeza via LLM ficam para
//! a v1.1+ ([ADR-0002]); a filtragem de alucinação (F003/FR-003-13) já acontece
//! no pós-processamento do motor (`managers::transcription::postprocess`), que
//! precisa da energia do áudio — aqui chega o texto já higienizado.
//!
//! O módulo é **puro**: sem `AppHandle`, sem I/O, sem relógio — toda a
//! configuração chega por [`PipelineInput`], o que permite testá-lo por tabela
//! (NFR-004-02). Quando a etapa de LLM chegar (v1.1+), as dependências
//! injetáveis (modelo, relógio, clipboard) entram via um `PipelineDeps` —
//! hoje não há nada a injetar.

mod cleanup;
mod dictionary;
mod normalize;
mod voice_commands;

pub use cleanup::default_light_filler_words;
pub use voice_commands::strip_trailing_submit_command;

use serde::{Deserialize, Serialize};
use specta::Type;
use std::collections::HashMap;

/// Limiar padrão da correção fuzzy do dicionário (`settings.word_correction_threshold`).
pub const DEFAULT_WORD_CORRECTION_THRESHOLD: f64 = 0.18;

/// Nível de limpeza do pipeline (FR-004-11 / data-model `text.cleanup_level`).
///
/// Na v1 não existe LLM no pipeline: `Medium`/`High` degradam para `Light`
/// (a UI já os esconde até a v1.1+). `None` aplica só as etapas
/// determinísticas 1, 2 e 4.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Type, Default)]
#[serde(rename_all = "snake_case")]
pub enum CleanupLevel {
    /// Nenhuma limpeza além das etapas determinísticas 1–4.
    None,
    /// Remove muletas pt-BR e repetições imediatas, corrige pontuação e
    /// capitalização (FR-004-12). Padrão.
    #[default]
    Light,
    /// `light` + backtrack, listas enumeradas e números/datas — v1.1+ (LLM).
    Medium,
    /// Reescrita para clareza no estilo do perfil — v1.1+ (LLM).
    High,
}

/// Frases que viram quebra de linha/parágrafo no ditado (FR-004-02),
/// configuráveis por idioma via `settings.voice_command_phrases`.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Type, Default)]
pub struct VoiceCommandPhrases {
    /// Frases que produzem `\n` (ex.: "nova linha", "new line").
    #[serde(default)]
    pub newline: Vec<String>,
    /// Frases que produzem `\n\n` (ex.: "novo parágrafo", "new paragraph").
    #[serde(default)]
    pub new_paragraph: Vec<String>,
}

/// Frases de quebra embutidas (FR-004-02): a tabela `"default"` vale para
/// qualquer idioma; `pt`/`en` acrescentam as formas do idioma (e `pt` cobre
/// `pt-BR`/`pt-PT` via resolução de prefixo).
pub fn default_voice_command_phrases() -> HashMap<String, VoiceCommandPhrases> {
    let strs = |list: &[&str]| list.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    HashMap::from([
        (
            "default".to_string(),
            VoiceCommandPhrases {
                newline: strs(&["nova linha", "new line"]),
                new_paragraph: strs(&["novo parágrafo", "new paragraph"]),
            },
        ),
        (
            "pt".to_string(),
            VoiceCommandPhrases {
                newline: strs(&["nova linha"]),
                new_paragraph: strs(&["novo parágrafo", "novo paragrafo"]),
            },
        ),
        (
            "en".to_string(),
            VoiceCommandPhrases {
                newline: strs(&["new line"]),
                new_paragraph: strs(&["new paragraph"]),
            },
        ),
    ])
}

/// Frases de "enviar" embutidas (FR-002-17 / FR-004-02): o ditado que termina
/// numa dessas frases sobe `press_enter` no [`PipelineOutput`]. A chave
/// `"default"` sempre aplica; `pt` cobre `pt-BR`.
pub fn default_voice_submit_phrases() -> HashMap<String, Vec<String>> {
    let strs = |list: &[&str]| list.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    HashMap::from([
        (
            "default".to_string(),
            strs(&["send", "press enter", "enviar"]),
        ),
        ("pt".to_string(), strs(&["enviar"])),
        ("en".to_string(), strs(&["send", "press enter", "send it"])),
    ])
}

/// Etapa executada, registrada no trace do pipeline — alimenta o detalhe
/// "cru × final" da sessão no histórico (F004, notas técnicas). O trace nunca
/// carrega conteúdo do texto (privacidade: nada de ditado em logs).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PipelineStage {
    /// Etapa 1 — normalização (FR-004-01).
    Normalize,
    /// Etapa 2 — comandos de voz (FR-004-02/03).
    VoiceCommands,
    /// Etapa 4 — dicionário, entradas `vocab` (FR-004-08).
    Dictionary,
    /// Etapa 5 — limpeza determinística `light` (FR-004-12).
    LightCleanup,
}

/// Registro de uma etapa: se o texto mudou. Sem conteúdo — a tela de detalhes
/// do histórico compara o texto cru com o final diretamente.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StageTrace {
    pub stage: PipelineStage,
    pub changed: bool,
}

/// Entrada do pipeline: o texto transcrito mais toda a configuração que as
/// etapas consultam. Imutável durante a execução — as etapas se encadeiam
/// sobre o texto.
#[derive(Debug, Clone)]
pub struct PipelineInput {
    /// Texto já transcrito (após o pós-processamento do motor).
    pub text: String,
    /// Idioma efetivo da saída ("pt-BR", "en"); `None` quando desconhecido
    /// ou `"auto"` — nesse caso só as frases `"default"` se aplicam.
    pub language: Option<String>,
    /// Nível de limpeza configurado (FR-004-11).
    pub cleanup_level: CleanupLevel,
    /// Lista de muletas da limpeza `light` (editável na tela Dicionário,
    /// T-044): `None` usa o padrão pt-BR embutido; `Some(vec![])` desliga a
    /// remoção de muletas — o resto da limpeza (repetições, pontuação,
    /// capitalização) continua.
    pub cleanup_filler_words: Option<Vec<String>>,
    /// Master switch herdado de `filler_word_removal_enabled`: quando `false`,
    /// as muletas listadas não são removidas (a correção de
    /// pontuação/capitalização da `light` continua).
    pub filler_removal_enabled: bool,
    /// Entradas `vocab` do dicionário (FR-004-08): também já usadas como dica
    /// ao STT; aqui a correção fuzzy é (re)aplicada — idempotente para quem
    /// já saiu corrigido do motor.
    pub custom_words: Vec<String>,
    /// Limiar da correção fuzzy do dicionário
    /// (`settings.word_correction_threshold`).
    pub word_correction_threshold: f64,
    /// Frases de comando de quebra por idioma (FR-004-02).
    pub voice_command_phrases: HashMap<String, VoiceCommandPhrases>,
    /// Frases de "enviar" no final do ditado (FR-002-17 / FR-004-02).
    pub voice_submit_phrases: HashMap<String, Vec<String>>,
    /// Pontuação falada ("vírgula", "ponto final"…): desligada por padrão
    /// (FR-004-03) porque os motores já pontuam.
    pub spoken_punctuation_enabled: bool,
}

impl Default for PipelineInput {
    /// Configuração "de fábrica": nível `light`, muletas pt-BR padrão, comandos
    /// de voz padrão. Testes devem sobrescrever `text`/`language`.
    fn default() -> Self {
        Self {
            text: String::new(),
            language: None,
            cleanup_level: CleanupLevel::default(),
            cleanup_filler_words: None,
            filler_removal_enabled: true,
            custom_words: Vec::new(),
            word_correction_threshold: DEFAULT_WORD_CORRECTION_THRESHOLD,
            voice_command_phrases: default_voice_command_phrases(),
            voice_submit_phrases: default_voice_submit_phrases(),
            spoken_punctuation_enabled: false,
        }
    }
}

/// Saída do pipeline: o texto final, a flag `press_enter` (F002 envia a tecla
/// após a inserção) e o trace por etapa.
#[derive(Debug, Clone)]
pub struct PipelineOutput {
    pub text: String,
    /// FR-004-02: o ditado terminou com um comando de envio ("… enviar").
    pub press_enter: bool,
    pub stages: Vec<StageTrace>,
}

/// Núcleo comparável de um token ou palavra de frase: só alfanuméricos,
/// minúsculos ("linha," → "linha", "Né" → "né"). Mesmo critério do
/// dicionário (`audio_toolkit::text`).
pub(crate) fn token_core(token: &str) -> String {
    token
        .chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(|c| c.to_lowercase())
        .collect()
}

/// Executa as etapas v1 do pipeline sobre o texto transcrito, na ordem da
/// spec: normalizar → comandos de voz → dicionário (vocab) → limpeza `light`.
pub fn run(input: &PipelineInput) -> PipelineOutput {
    let mut stages = Vec::with_capacity(4);
    let mut text = input.text.clone();

    let next = normalize::run(&text);
    stages.push(StageTrace {
        stage: PipelineStage::Normalize,
        changed: next != text,
    });
    text = next;

    let (next, press_enter) = voice_commands::run(&text, input);
    stages.push(StageTrace {
        stage: PipelineStage::VoiceCommands,
        changed: next != text || press_enter,
    });
    text = next;

    let next = dictionary::run(&text, &input.custom_words, input.word_correction_threshold);
    stages.push(StageTrace {
        stage: PipelineStage::Dictionary,
        changed: next != text,
    });
    text = next;

    // FR-004-11/12: `none` pula a etapa; `medium`/`high` exigem LLM (v1.1+) e
    // na v1 degradam para a `light` determinística.
    if input.cleanup_level != CleanupLevel::None {
        let next = cleanup::light(&text, input);
        stages.push(StageTrace {
            stage: PipelineStage::LightCleanup,
            changed: next != text,
        });
        text = next;
    }

    PipelineOutput {
        text: text.trim().to_string(),
        press_enter,
        stages,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pt_input(text: &str) -> PipelineInput {
        PipelineInput {
            text: text.to_string(),
            language: Some("pt-BR".to_string()),
            ..Default::default()
        }
    }

    /// AC-004-01: "é, tipo, eu acho que a gente pode, né, fazer amanhã" no
    /// nível `light` sai "Eu acho que a gente pode fazer amanhã."
    #[test]
    fn ac_004_01_light_cleanup_removes_muletas() {
        let out = run(&pt_input(
            "é, tipo, eu acho que a gente pode, né, fazer amanhã",
        ));
        assert_eq!(out.text, "Eu acho que a gente pode fazer amanhã.");
        assert!(!out.press_enter);
        assert!(out
            .stages
            .iter()
            .any(|s| s.stage == PipelineStage::LightCleanup && s.changed));
    }

    /// AC-004-10: "primeira linha nova linha segunda linha" sai
    /// "Primeira linha\nSegunda linha".
    #[test]
    fn ac_004_10_newline_command_breaks_line() {
        let out = run(&pt_input("primeira linha nova linha segunda linha"));
        assert_eq!(out.text, "Primeira linha\nSegunda linha");
    }

    /// FR-004-02: "enviar" no final vira `press_enter` e sai do texto.
    #[test]
    fn trailing_send_command_sets_press_enter() {
        let out = run(&pt_input("vou chegar em cinco minutos enviar"));
        assert_eq!(out.text, "Vou chegar em cinco minutos.");
        assert!(out.press_enter);
    }

    /// FR-004-11 `none`: só as etapas determinísticas — muletas sobrevivem.
    #[test]
    fn cleanup_level_none_keeps_fillers() {
        let out = run(&PipelineInput {
            cleanup_level: CleanupLevel::None,
            ..pt_input("isso aí é, né, importante")
        });
        // "é" fica por ser palavra real; muletas não são removidas.
        assert_eq!(out.text, "Isso aí é, né, importante");
    }

    /// FR-004-08: entradas `vocab` corrigem a grafia do texto final — o
    /// n-gram "transcreve ai" casa exato com o termo "Transcreve.ai".
    #[test]
    fn dictionary_stage_applies_vocab_corrections() {
        let out = run(&PipelineInput {
            custom_words: vec!["Transcreve.ai".to_string()],
            ..pt_input("eu uso transcreve ai todo dia")
        });
        assert_eq!(out.text, "Eu uso Transcreve.ai todo dia.");
    }

    /// Pipeline sobre texto vazio/espaços: sai vazio sem pânico.
    #[test]
    fn empty_input_stays_empty() {
        let out = run(&pt_input("   "));
        assert_eq!(out.text, "");
        assert!(!out.press_enter);
    }

    /// Níveis v1.1+ degradam para `light` determinística na v1.
    #[test]
    fn medium_and_high_degrade_to_light() {
        for level in [CleanupLevel::Medium, CleanupLevel::High] {
            let out = run(&PipelineInput {
                cleanup_level: level,
                ..pt_input("isso, né, funcionou")
            });
            assert_eq!(out.text, "Isso funcionou.");
        }
    }
}
