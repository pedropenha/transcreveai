//! Hardware detection and per-model suitability labels (FR-003-05).
//!
//! The spec asks for a first-run hardware probe — RAM, core count, AVX2, and a
//! Vulkan/CUDA-class GPU with its VRAM — whose only job is to inform
//! *suitability labels* on the model list ("pesado para esta máquina" below
//! 8 GB RAM, …). It never imposes a choice: `large-v3-turbo` stays the
//! recommended default and the user picks freely in onboarding.
//!
//! Everything label-related is a pure function so the thresholds are unit-test
//! covered; only [`detect_hardware`] touches the OS.

use crate::managers::model::ModelInfo;
use serde::Serialize;
use specta::Type;

/// Coarse machine class, the row selector of FR-003-05's table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum HardwareTier {
    /// A GPU usable by transcribe.cpp (Vulkan/Metal) with >= 4 GB VRAM.
    Gpu,
    /// Modern CPU (AVX2-class SIMD) with >= 16 GB RAM.
    CapableCpu,
    /// >= 8 GB RAM but neither of the above.
    Modest,
    /// Below 8 GB RAM — the UI also surfaces this as a "weak hardware" warning.
    Weak,
}

/// What the OS probe found. Serialized to the frontend for diagnostics and the
/// suitability labels' tooltips.
#[derive(Debug, Clone, Serialize, Type)]
pub struct HardwareReport {
    pub total_ram_mb: u64,
    pub cpu_cores: u32,
    /// AVX2 on x86 hosts. Reported as `true` on aarch64, where NEON is a
    /// baseline guarantee — the field answers "does this CPU have modern SIMD",
    /// which is what the tier table actually needs.
    pub has_avx2: bool,
    /// Names of the GPU devices transcribe.cpp can use (Vulkan/Metal).
    pub gpu_names: Vec<String>,
    /// VRAM of the largest usable GPU; 0 when no GPU or the backend does not
    /// report capacity (e.g. Metal on unified-memory Apple Silicon).
    pub max_gpu_vram_mb: u64,
    /// The GPU execution provider the ONNX stack can use on this build/host
    /// ("coreml" on macOS, "directml" on Windows x64), when one is compiled in.
    /// `None` means ONNX-family engines (Parakeet, Moonshine, …) run CPU-bound.
    pub ort_gpu_accelerator: Option<String>,
    pub tier: HardwareTier,
}

/// The per-model label FR-003-05 defines. Rendered as a badge/hint; it never
/// blocks selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum Suitability {
    /// The machine-appropriate default pick.
    Recommended,
    /// Expected to run well.
    GoodFit,
    /// Runs, but heavy for this machine.
    Heavy,
    /// Unlikely to run acceptably here.
    NotAdvised,
}

/// One model's suitability label, joined to the model list by `model_id`.
#[derive(Debug, Clone, Serialize, Type)]
pub struct ModelSuitabilityEntry {
    pub model_id: String,
    pub label: Suitability,
    /// Whether this model's engine can run GPU-bound on the probed machine.
    pub gpu_accelerated: bool,
}

/// The whole "what does this machine look like and what should it run" bundle
/// the model picker consumes in a single call.
#[derive(Debug, Clone, Serialize, Type)]
pub struct ModelRecommendations {
    pub hardware: HardwareReport,
    /// The first-run default (`large-v3-turbo`) as a registry id, when the
    /// catalog knows it. The UI preselects this without ever being forced to.
    pub default_model_id: Option<String>,
    pub labels: Vec<ModelSuitabilityEntry>,
}

const DEDICATED_GPU_VRAM_MB: u64 = 4 * 1024;
const CAPABLE_CPU_RAM_MB: u64 = 16 * 1024;
const MODEST_RAM_MB: u64 = 8 * 1024;

/// Rough per-model weight class used by the suitability table, keyed off the
/// download size: <=300 MB covers tiny/base-class whisper quants and the small
/// ONNX models; <=1400 MB covers small/medium/turbo-class weights; beyond that
/// (cohere, Voxtral, large Q8+) is heavyweight.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WeightClass {
    Light,
    Medium,
    Heavy,
}

fn weight_class(size_mb: u64) -> WeightClass {
    match size_mb {
        0..=300 => WeightClass::Light,
        301..=1400 => WeightClass::Medium,
        _ => WeightClass::Heavy,
    }
}

/// Whether a registry entry is the `large-v3-turbo` model — the catalog GGUF
/// entry (`…/whisper-large-v3-turbo-Q8_0.gguf`) or the legacy `turbo` .bin.
fn is_turbo_model(model: &ModelInfo) -> bool {
    model.id == "turbo"
        || model.id.contains("large-v3-turbo")
        || model.filename.contains("large-v3-turbo")
}

/// The machine's tier per FR-003-05's table. `gpu_present` matters separately
/// from `max_gpu_vram_mb` because backends that can't report VRAM (Metal on
/// unified memory) leave it at 0: such a GPU counts as dedicated-class only
/// when there's enough shared RAM to back it.
pub fn hardware_tier(
    total_ram_mb: u64,
    has_avx2: bool,
    gpu_present: bool,
    max_gpu_vram_mb: u64,
) -> HardwareTier {
    let gpu_capable = max_gpu_vram_mb >= DEDICATED_GPU_VRAM_MB
        || (gpu_present && max_gpu_vram_mb == 0 && total_ram_mb >= CAPABLE_CPU_RAM_MB);
    if gpu_capable {
        HardwareTier::Gpu
    } else if total_ram_mb >= CAPABLE_CPU_RAM_MB && has_avx2 {
        HardwareTier::CapableCpu
    } else if total_ram_mb >= MODEST_RAM_MB {
        HardwareTier::Modest
    } else {
        HardwareTier::Weak
    }
}

/// The suitability label for one model on a given tier (FR-003-05's table).
/// Pure over the tier and size so the thresholds are exhaustively testable.
pub fn suitability_label(tier: HardwareTier, size_mb: u64, is_turbo: bool) -> Suitability {
    // large-v3-turbo is the pinned default: recommended on GPU or a modern
    // >=16 GB CPU, labelled "heavy" below that rather than silently demoted.
    if is_turbo {
        return match tier {
            HardwareTier::Gpu | HardwareTier::CapableCpu => Suitability::Recommended,
            HardwareTier::Modest | HardwareTier::Weak => Suitability::Heavy,
        };
    }
    match (tier, weight_class(size_mb)) {
        // <8 GB: tiny/base-class models are the pick; warn about the rest.
        (HardwareTier::Weak, WeightClass::Light) => Suitability::Recommended,
        (HardwareTier::Weak, WeightClass::Medium) => Suitability::Heavy,
        (HardwareTier::Weak, WeightClass::Heavy) => Suitability::NotAdvised,
        // 8–16 GB: base/small-class suggested; heavyweight models not advised.
        (HardwareTier::Modest, WeightClass::Heavy) => Suitability::NotAdvised,
        (HardwareTier::Modest, _) => Suitability::GoodFit,
        // A capable GPU shrugs at anything that fits in RAM at all.
        (HardwareTier::Gpu, _) => Suitability::GoodFit,
        (HardwareTier::CapableCpu, WeightClass::Heavy) => Suitability::Heavy,
        (HardwareTier::CapableCpu, _) => Suitability::GoodFit,
    }
}

/// Label a model for a machine: [`suitability_label`] plus a hard floor — a
/// model whose weights alone approach/exceed total RAM cannot transcribe
/// acceptably however strong the GPU, so it is always `NotAdvised`.
pub fn model_suitability(model: &ModelInfo, hardware: &HardwareReport) -> Suitability {
    if model.size_mb.saturating_mul(2) > hardware.total_ram_mb {
        return Suitability::NotAdvised;
    }
    suitability_label(hardware.tier, model.size_mb, is_turbo_model(model))
}

/// Whether `model`'s engine has a GPU path on this machine: transcribe.cpp
/// engines (Whisper, GGUF Parakeet, …) use the enumerated Vulkan/Metal
/// devices; ONNX engines (Parakeet, Moonshine, …) use the compiled-in ORT
/// execution provider (CoreML on macOS, DirectML on Windows x64).
pub fn model_gpu_accelerated(model: &ModelInfo, hardware: &HardwareReport) -> bool {
    match model.engine_type {
        crate::managers::model::EngineType::TranscribeCpp => !hardware.gpu_names.is_empty(),
        _ => hardware.ort_gpu_accelerator.is_some(),
    }
}

/// Build the full recommendations bundle for the current registry.
pub fn recommendations_for(
    models: &[ModelInfo],
    hardware: &HardwareReport,
) -> ModelRecommendations {
    ModelRecommendations {
        hardware: hardware.clone(),
        // The first-run default is the catalog's turbo entry — resolved from
        // the catalog itself so the id stays correct if quants are added.
        default_model_id: crate::catalog::CATALOG
            .iter()
            .find(|d| d.id.contains("large-v3-turbo"))
            .map(|d| d.id.clone()),
        labels: models
            .iter()
            .map(|m| ModelSuitabilityEntry {
                model_id: m.id.clone(),
                label: model_suitability(m, hardware),
                gpu_accelerated: model_gpu_accelerated(m, hardware),
            })
            .collect(),
    }
}

/// Total physical RAM in MB, or 0 when the platform API can't say.
/// Detection failure must not label anything: `0` lands the machine in `Weak`,
/// and callers treat a 0-VRAM GPU the same way (conservative).
fn total_ram_mb() -> u64 {
    imp::total_ram_mb()
}

/// AVX2 on x86; unconditionally true on aarch64 where NEON is baseline. Other
/// architectures report false and are tiered conservatively.
fn has_modern_simd() -> bool {
    #[cfg(any(target_arch = "x86_64", target_arch = "x86"))]
    {
        std::arch::is_x86_feature_detected!("avx2")
    }
    #[cfg(target_arch = "aarch64")]
    {
        true
    }
    #[cfg(not(any(target_arch = "x86_64", target_arch = "x86", target_arch = "aarch64")))]
    {
        false
    }
}

fn cpu_cores() -> u32 {
    std::thread::available_parallelism()
        .map(|n| n.get() as u32)
        .unwrap_or(1)
}

/// Probe the machine. GPU enumeration reuses the transcription engine's
/// cached device list (transcribe.cpp's Vulkan/Metal backends — already
/// pre-warmed at startup), so this is cheap after first call.
pub fn detect_hardware() -> HardwareReport {
    let accelerators = crate::managers::transcription::get_available_accelerators();
    let gpu_names: Vec<String> = accelerators
        .gpu_devices
        .iter()
        .map(|d| d.name.clone())
        .collect();
    let max_gpu_vram_mb = accelerators
        .gpu_devices
        .iter()
        .map(|d| d.total_vram_mb as u64)
        .max()
        .unwrap_or(0);
    let total_ram_mb = total_ram_mb();
    let has_avx2 = has_modern_simd();
    HardwareReport {
        total_ram_mb,
        cpu_cores: cpu_cores(),
        has_avx2,
        tier: hardware_tier(
            total_ram_mb,
            has_avx2,
            !gpu_names.is_empty(),
            max_gpu_vram_mb,
        ),
        gpu_names,
        max_gpu_vram_mb,
        ort_gpu_accelerator: accelerators.ort.into_iter().find(|ep| ep != "cpu"),
    }
}

#[cfg(target_os = "windows")]
mod imp {
    /// `GlobalMemoryStatusEx` — no version gate needed, available since XP.
    pub fn total_ram_mb() -> u64 {
        use windows::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
        let mut status = MEMORYSTATUSEX {
            dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32,
            ..Default::default()
        };
        // SAFETY: `status` is a valid, correctly-sized MEMORYSTATUSEX out-param.
        match unsafe { GlobalMemoryStatusEx(&mut status) } {
            Ok(()) => status.ullTotalPhys / (1024 * 1024),
            Err(_) => 0,
        }
    }
}

#[cfg(target_os = "linux")]
mod imp {
    /// `MemTotal` from /proc/meminfo (reported in kB).
    pub fn total_ram_mb() -> u64 {
        std::fs::read_to_string("/proc/meminfo")
            .ok()
            .and_then(|contents| {
                contents
                    .lines()
                    .find(|line| line.starts_with("MemTotal:"))
                    .and_then(|line| {
                        line.split_whitespace()
                            .nth(1)
                            .and_then(|kb| kb.parse::<u64>().ok())
                    })
            })
            .unwrap_or(0)
            / 1024
    }
}

#[cfg(target_os = "macos")]
mod imp {
    /// `hw.memsize` via sysctlbyname.
    pub fn total_ram_mb() -> u64 {
        let mut bytes: u64 = 0;
        let mut len = std::mem::size_of::<u64>();
        // SAFETY: `bytes`/`len` are valid out-params for a hw.memsize u64.
        let rc = unsafe {
            libc::sysctlbyname(
                c"hw.memsize".as_ptr(),
                &mut bytes as *mut u64 as *mut _,
                &mut len,
                std::ptr::null_mut(),
                0,
            )
        };
        if rc == 0 {
            bytes / (1024 * 1024)
        } else {
            0
        }
    }
}

#[cfg(not(any(target_os = "windows", target_os = "linux", target_os = "macos")))]
mod imp {
    pub fn total_ram_mb() -> u64 {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::managers::model::{EngineType, ModelSource};

    const GB: u64 = 1024;

    fn model(id: &str, size_mb: u64) -> ModelInfo {
        ModelInfo {
            id: id.to_string(),
            name: id.to_string(),
            description: String::new(),
            filename: format!("{id}.gguf"),
            source: ModelSource::Local,
            size_mb,
            is_downloaded: true,
            is_downloading: false,
            partial_size: 0,
            is_directory: false,
            engine_type: EngineType::TranscribeCpp,
            accuracy_score: 0.0,
            speed_score: 0.0,
            supports_translation: false,
            is_recommended: false,
            supported_languages: vec![],
            supports_language_selection: false,
            is_custom: true,
            supports_streaming: false,
            supports_language_detection: false,
        }
    }

    fn hw(total_ram_mb: u64, has_avx2: bool, gpu_present: bool, vram_mb: u64) -> HardwareReport {
        HardwareReport {
            total_ram_mb,
            cpu_cores: 8,
            has_avx2,
            gpu_names: if gpu_present {
                vec!["gpu".to_string()]
            } else {
                vec![]
            },
            max_gpu_vram_mb: vram_mb,
            ort_gpu_accelerator: None,
            tier: hardware_tier(total_ram_mb, has_avx2, gpu_present, vram_mb),
        }
    }

    #[test]
    fn tier_table_matches_fr_003_05() {
        // Dedicated GPU >= 4 GB VRAM wins outright.
        assert_eq!(hardware_tier(8 * GB, true, true, 4 * GB), HardwareTier::Gpu);
        assert_eq!(
            hardware_tier(8 * GB, true, true, 4 * GB - 1),
            HardwareTier::Modest
        );
        // Unified-memory GPU (no reported VRAM) counts only with enough RAM.
        assert_eq!(hardware_tier(16 * GB, false, true, 0), HardwareTier::Gpu);
        assert_eq!(hardware_tier(8 * GB, false, true, 0), HardwareTier::Modest);
        // Modern CPU needs >=16 GB *and* AVX2-class SIMD.
        assert_eq!(
            hardware_tier(16 * GB, true, false, 0),
            HardwareTier::CapableCpu
        );
        assert_eq!(
            hardware_tier(16 * GB, false, false, 0),
            HardwareTier::Modest
        );
        assert_eq!(hardware_tier(8 * GB, true, false, 0), HardwareTier::Modest);
        assert_eq!(
            hardware_tier(8 * GB - 1, true, false, 0),
            HardwareTier::Weak
        );
    }

    #[test]
    fn turbo_is_the_recommended_default_on_capable_hardware() {
        for tier in [HardwareTier::Gpu, HardwareTier::CapableCpu] {
            assert_eq!(suitability_label(tier, 886, true), Suitability::Recommended);
        }
        // Below 8–16 GB the same model is labelled heavy, never hidden.
        assert_eq!(
            suitability_label(HardwareTier::Modest, 886, true),
            Suitability::Heavy
        );
        assert_eq!(
            suitability_label(HardwareTier::Weak, 886, true),
            Suitability::Heavy
        );
    }

    #[test]
    fn weak_hardware_suggests_light_models() {
        assert_eq!(
            suitability_label(HardwareTier::Weak, 85, false),
            Suitability::Recommended
        );
        assert_eq!(
            suitability_label(HardwareTier::Modest, 85, false),
            Suitability::GoodFit
        );
        assert_eq!(
            suitability_label(HardwareTier::Modest, 25_000, false),
            Suitability::NotAdvised
        );
        assert_eq!(
            suitability_label(HardwareTier::CapableCpu, 2_000, false),
            Suitability::Heavy
        );
    }

    #[test]
    fn weights_exceeding_ram_are_never_advised() {
        // 25 GB of weights can't mmap comfortably on a 16 GB machine, GPU or not.
        let big = model("voxtral-small", 25_000);
        let gpu_hw = hw(16 * GB, true, true, 24 * GB);
        assert_eq!(model_suitability(&big, &gpu_hw), Suitability::NotAdvised);
        // …but it is fine where it fits.
        let roomy = hw(64 * GB, true, true, 24 * GB);
        assert_eq!(model_suitability(&big, &roomy), Suitability::GoodFit);
    }

    #[test]
    fn is_turbo_model_matches_catalog_and_legacy_ids() {
        assert!(is_turbo_model(&model(
            "handy-computer/whisper-large-v3-turbo-gguf/whisper-large-v3-turbo-Q8_0.gguf",
            886
        )));
        assert!(is_turbo_model(&model("turbo", 1549)));
        assert!(!is_turbo_model(&model("small", 465)));
    }

    #[test]
    fn gpu_acceleration_flag_tracks_engine_and_host() {
        let mut whisper = model("whisper-turbo", 886);
        whisper.engine_type = EngineType::TranscribeCpp;
        let mut onnx = model("parakeet", 500);
        onnx.engine_type = EngineType::Parakeet;

        // GPU host with no ORT EP: whisper accelerates, ONNX stays CPU-bound.
        let host = hw(16 * GB, true, true, 8 * GB);
        assert!(model_gpu_accelerated(&whisper, &host));
        assert!(!model_gpu_accelerated(&onnx, &host));

        // Same host with an ORT GPU EP compiled in (CoreML/DirectML): both go.
        let mut onnx_host = host.clone();
        onnx_host.ort_gpu_accelerator = Some("coreml".to_string());
        assert!(model_gpu_accelerated(&onnx, &onnx_host));

        // No GPU devices and no EP: nothing accelerates.
        let cpu_host = hw(16 * GB, true, false, 0);
        assert!(!model_gpu_accelerated(&whisper, &cpu_host));
        assert!(!model_gpu_accelerated(&onnx, &cpu_host));
    }

    #[test]
    fn recommendations_carry_catalog_default() {
        let models = vec![model("a", 100), model("b", 900)];
        let recs = recommendations_for(&models, &hw(8 * GB, true, false, 0));
        assert_eq!(recs.labels.len(), 2);
        // The default is the catalog's turbo entry (bundled catalog always has one).
        let default = recs.default_model_id.expect("catalog has turbo");
        assert!(default.contains("large-v3-turbo"));
        assert_eq!(recs.hardware.tier, HardwareTier::Modest);
    }
}
