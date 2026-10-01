// Re-export all audio components
mod device;
/// WAV-file audio source for E2E test builds (T-009). Compiled out of normal
/// builds; see the module docs for the `TRANSCREVE_AUDIO_FIXTURE` contract.
#[cfg(feature = "audio-fixture")]
pub(crate) mod fixture;
/// System-audio (WASAPI loopback) capture for the meeting notetaker (T-063).
/// `pub` so `crate::meeting` can inject the backend trait for tests.
pub mod loopback;
mod recorder;
mod resampler;
mod utils;
mod visualizer;

pub use device::{list_input_devices, list_output_devices, CpalDeviceInfo};
pub use loopback::{DetachReason, LoopbackBackend, SystemAudioCapture, SystemAudioEvent};
pub use recorder::{
    is_microphone_access_denied, is_no_input_device_error, AudioFrameCallback, AudioRecorder,
    FrameSubscriber, FrameTap, RecordedClip, VadPolicy,
};
pub use resampler::{FrameResampler, ResamplerInitError};
pub use utils::{read_wav_samples, save_wav_file, verify_wav_file};
pub use visualizer::AudioVisualiser;
