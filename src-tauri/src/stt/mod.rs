//! Fronteira de provedores STT (speech-to-text) — `specs/features/003` e
//! `specs/architecture/contracts.md` §1–2.
//!
//! * [`types`] — `ProviderId`, `AudioBuffer`, `SttOptions`, `Transcript`,
//!   `SttError` e demais tipos do contrato.
//! * [`provider`] — o trait [`provider::SttProvider`].
//! * [`orchestrator`] — seleção de provedor, retry único e fallback
//!   (FR-003-16), independente de Tauri e testável com providers mockados.
//! * [`local`] — `LocalSttProvider`: adapta os motores locais (whisper.cpp /
//!   ONNX) ao trait.
//!
//! Parte desta superfície é contrato pensado para os provedores de nuvem da
//! v1.1+ (`capabilities`, `health_check`, `Segment`, variantes de `SttError`
//! como `Auth`/`RateLimited`): itens ainda sem chamador na v1 levam
//! `#[allow(dead_code)]` pontual e documentado.

pub mod local;
pub mod orchestrator;
pub mod provider;
pub mod selection;
pub mod types;
