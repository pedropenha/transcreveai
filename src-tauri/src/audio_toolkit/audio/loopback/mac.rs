//! macOS system-audio capture via ScreenCaptureKit (T-106, FR-009-03/04).
//!
//! SCK taps *process* audio system-wide: unlike the WASAPI loopback it is not
//! bound to the render endpoint, so plugging/unplugging a headset does not
//! interrupt the track and [`LoopbackBackend::default_device_name`] reports a
//! constant virtual endpoint. `open` fails — and the worker retries with
//! backoff — until the user grants Screen Recording, which SCK audio capture
//! requires on macOS 13+.

use std::ptr::{null_mut, NonNull};
use std::sync::atomic::Ordering;
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

use block2::RcBlock;
use dispatch2::{DispatchQueue, DispatchQueueAttr, DispatchRetained};
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::{define_class, AnyThread, DefinedClass};
use objc2_core_audio_types::AudioBufferList;
use objc2_core_foundation::CFRetained;
use objc2_core_graphics::{CGPreflightScreenCaptureAccess, CGRequestScreenCaptureAccess};
use objc2_core_media::{
    kCMSampleBufferFlag_AudioBufferList_Assure16ByteAlignment, CMBlockBuffer, CMSampleBuffer,
    CMTime,
};
use objc2_foundation::{NSArray, NSError, NSObject, NSObjectProtocol};
use objc2_screen_capture_kit::{
    SCContentFilter, SCShareableContent, SCStream, SCStreamConfiguration, SCStreamDelegate,
    SCStreamOutput, SCStreamOutputType,
};
use rtrb::{Producer, RingBuffer};

use super::{LoopbackBackend, LoopbackStream, OpenedLoopback, RING_SECONDS};
use crate::audio_toolkit::audio::recorder::{AudioRecorder, CaptureTransportState};

/// Virtual endpoint name — the SCK tap is system-wide, so there is no device
/// to reattach to. Kept constant so [`assess_stream`](super::assess_stream)
/// never reports `DefaultDeviceChanged`.
const DEVICE_NAME: &str = "ScreenCaptureKit system audio";

/// SCK audio is delivered float32 at this rate (monotonic with
/// `SCStreamConfiguration.sampleRate` below); the worker's `FrameResampler`
/// down-samples to 16 kHz.
const SAMPLE_RATE: u32 = 48_000;
/// Channels requested from SCK; the planar mixdown to mono reuses
/// `AudioRecorder::write_input_to_ring`.
const CHANNELS: u32 = 2;
/// Shareable-content and start-capture completion handlers run on SCK
/// internal queues; `open` parks the worker thread waiting on a channel —
/// never on the audio path.
const COMPLETION_TIMEOUT: Duration = Duration::from_secs(10);

/// Error text for the permission wall — surfaced through
/// `DetachReason::OpenFailed` so the session can warn the user instead of
/// silently transcribing mic only.
fn permission_denied() -> String {
    "Screen Recording permission required — enable Transcreve.ai in System Settings › Privacy & Security › Screen Recording".to_string()
}

fn describe(error: *mut NSError) -> String {
    NonNull::new(error)
        .map(|err| unsafe { err.as_ref().localizedDescription().to_string() })
        .unwrap_or_else(|| "unknown ScreenCaptureKit error".to_string())
}

/// Ivars of the SCK handler object: the audio callback and the delegate live
/// on one object because SCK calls both on its own queues.
pub struct HandlerIvars {
    /// SPSC producer feeding the worker's ring. The sample-handler queue is
    /// serial, so the mutex is never contended — it exists because the
    /// callback signature takes `&self`.
    producer: Mutex<Producer<f32>>,
    transport: Arc<CaptureTransportState>,
    /// `stream:didStopWithError:` result, polled by `LoopbackStream::failure`.
    failure: Arc<Mutex<Option<String>>>,
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements; ivars are plain Rust
    // values behind Mutex.
    #[unsafe(super(NSObject))]
    #[name = "TranscreveLoopbackHandler"]
    #[ivars = HandlerIvars]
    pub struct LoopbackHandler;

    unsafe impl NSObjectProtocol for LoopbackHandler {}

    unsafe impl SCStreamOutput for LoopbackHandler {
        /// SCK audio callback (serial `sampleHandlerQueue`): pull the PCM out
        /// of the CMSampleBuffer and push it into the worker's ring. Any
        /// failure only drops that buffer — a hard stop arrives through
        /// `stream:didStopWithError:`.
        #[unsafe(method(stream:didOutputSampleBuffer:ofType:))]
        unsafe fn stream_did_output_sample_buffer(
            &self,
            _stream: &SCStream,
            sample_buffer: &CMSampleBuffer,
            output_type: SCStreamOutputType,
        ) {
            if output_type != SCStreamOutputType::Audio {
                return;
            }
            self.push_samples(sample_buffer);
        }
    }

    unsafe impl SCStreamDelegate for LoopbackHandler {
        /// The OS stopped the stream (permission revoked, SCK error): the
        /// worker's next `failure()` poll detaches and the backoff loop
        /// reopens — matching the Windows detach/reattach contract.
        #[unsafe(method(stream:didStopWithError:))]
        unsafe fn stream_did_stop_with_error(&self, _stream: &SCStream, error: &NSError) {
            let message = error.localizedDescription().to_string();
            log::warn!("ScreenCaptureKit stream stopped: {message}");
            if let Ok(mut slot) = self.ivars().failure.lock() {
                *slot = Some(message);
            }
        }
    }
);

impl LoopbackHandler {
    /// Pull float32 PCM out of `sample_buffer`'s AudioBufferList and push
    /// mono-mixed frames into the ring with the mic path's overrun
    /// accounting. SCK delivers non-interleaved f32 (one AudioBuffer per
    /// channel); a single interleaved buffer is also accepted.
    fn push_samples(&self, sample_buffer: &CMSampleBuffer) {
        // First call with a null list asks for the required size.
        let mut needed = 0usize;
        let status = unsafe {
            sample_buffer.audio_buffer_list_with_retained_block_buffer(
                &mut needed,
                null_mut(),
                0,
                None,
                None,
                0,
                null_mut(),
            )
        };
        if status != 0 || needed == 0 {
            return;
        }
        let mut storage = vec![0u8; needed];
        let mut block_raw: *mut CMBlockBuffer = null_mut();
        let status = unsafe {
            sample_buffer.audio_buffer_list_with_retained_block_buffer(
                null_mut(),
                storage.as_mut_ptr().cast(),
                needed,
                None,
                None,
                kCMSampleBufferFlag_AudioBufferList_Assure16ByteAlignment,
                &mut block_raw,
            )
        };
        // `retained_block_buffer` hands ownership: release at scope end.
        let _block = NonNull::new(block_raw).map(|ptr| unsafe { CFRetained::from_raw(ptr) });
        if status != 0 {
            return;
        }
        let list = unsafe { &*storage.as_ptr().cast::<AudioBufferList>() };
        let count = list.mNumberBuffers as usize;
        if count == 0 {
            return;
        }
        let buffers = unsafe { std::slice::from_raw_parts(list.mBuffers.as_ptr(), count) };

        let ivars = self.ivars();
        let mut producer = match ivars.producer.lock() {
            Ok(p) => p,
            Err(e) => e.into_inner(),
        };
        if buffers.iter().all(|b| b.mNumberChannels == 1) {
            // Planar float32: one AudioBuffer per channel — interleave, then
            // let the shared ring path mono-mix and count overruns.
            let frames = (buffers[0].mDataByteSize / 4) as usize;
            if frames == 0 || !buffers.iter().all(|b| !b.mData.is_null()) {
                return;
            }
            let mut interleaved = vec![0f32; frames * count];
            for (ch, buf) in buffers.iter().enumerate() {
                let plane = unsafe { std::slice::from_raw_parts(buf.mData.cast::<f32>(), frames) };
                for (frame, &sample) in plane.iter().enumerate() {
                    interleaved[frame * count + ch] = sample;
                }
            }
            AudioRecorder::write_input_to_ring(
                &interleaved,
                count,
                None,
                &mut producer,
                &ivars.transport,
            );
        } else if let [buf] = buffers {
            // Single interleaved buffer — feed it through unchanged.
            let channels = buf.mNumberChannels as usize;
            if channels == 0 || buf.mData.is_null() {
                return;
            }
            let floats = (buf.mDataByteSize / 4) as usize;
            let data = unsafe { std::slice::from_raw_parts(buf.mData.cast::<f32>(), floats) };
            AudioRecorder::write_input_to_ring(
                data,
                channels,
                None,
                &mut producer,
                &ivars.transport,
            );
        }
    }
}

/// Production [`LoopbackBackend`] on macOS: a ScreenCaptureKit audio tap over
/// every shareable application.
pub(super) struct SckLoopbackBackend;

impl LoopbackBackend for SckLoopbackBackend {
    /// Constant virtual endpoint: SCK captures process audio regardless of
    /// which output device is current, so there is nothing to reattach to.
    fn default_device_name(&self) -> Result<Option<String>, String> {
        Ok(Some(DEVICE_NAME.to_string()))
    }

    fn open(&self) -> Result<OpenedLoopback, String> {
        // The framework is weak-linked (minimum macOS is 10.15): touching an
        // SCK class on <13 would panic in the generated class lookup.
        if objc2::runtime::AnyClass::get(c"SCStream").is_none() {
            return Err("System audio capture requires macOS 13 or newer".to_string());
        }
        // Preflight first so the failure message names the missing
        // permission instead of surfacing a cryptic SCK error.
        if !CGPreflightScreenCaptureAccess() && !CGRequestScreenCaptureAccess() {
            return Err(permission_denied());
        }

        // Screens/apps inventory. The completion handler runs on an SCK
        // queue; `open` blocks the worker — never the caller's main thread.
        let (tx, rx) = mpsc::channel();
        let block = RcBlock::new(
            move |content: *mut SCShareableContent, error: *mut NSError| {
                let result = if error.is_null() {
                    unsafe { Retained::retain_autoreleased(content) }
                        .ok_or_else(|| "ScreenCaptureKit returned no shareable content".to_string())
                } else {
                    Err(describe(error))
                };
                let _ = tx.send(result);
            },
        );
        unsafe {
            SCShareableContent::getShareableContentExcludingDesktopWindows_onScreenWindowsOnly_completionHandler(
                true,
                false,
                &block,
            );
        }
        let content = rx
            .recv_timeout(COMPLETION_TIMEOUT)
            .map_err(|_| "Timed out reading ScreenCaptureKit shareable content".to_string())??;

        // Filter: every application on the main display, no window
        // exceptions — audio is system-wide for the included processes.
        let displays = unsafe { content.displays() };
        let display = displays
            .firstObject()
            .ok_or_else(|| "No display found for system audio capture".to_string())?;
        let empty: Retained<NSArray<objc2_screen_capture_kit::SCWindow>> = NSArray::new();
        let filter = unsafe {
            SCContentFilter::initWithDisplay_includingApplications_exceptingWindows(
                SCContentFilter::alloc(),
                &display,
                &content.applications(),
                &empty,
            )
        };

        let config = unsafe { SCStreamConfiguration::new() };
        unsafe {
            config.setCapturesAudio(true);
            // Meeting transcription must never echo the user's own output
            // (feedback sounds, TTS) into the "Outros" track.
            config.setExcludesCurrentProcessAudio(true);
            config.setSampleRate(SAMPLE_RATE as _);
            config.setChannelCount(CHANNELS as _);
            // Screen output is never attached; keep the mandatory geometry
            // minimal so the video pipeline idles.
            config.setWidth(2);
            config.setHeight(2);
            config.setMinimumFrameInterval(CMTime::new(1, 1));
            config.setShowsCursor(false);
        }

        let capacity = SAMPLE_RATE as usize * RING_SECONDS;
        let (mut producer, mut consumer) = RingBuffer::new(capacity);
        // Touch the ring pages up front like the cpal path does.
        {
            let chunk = producer
                .write_chunk(capacity)
                .expect("new audio ring has its full capacity available");
            chunk.commit_all();
        }
        {
            let chunk = consumer
                .read_chunk(capacity)
                .expect("pre-filled audio ring is readable");
            chunk.commit_all();
        }

        let transport = Arc::new(CaptureTransportState::default());
        let failure = Arc::new(Mutex::new(None));
        let handler: Retained<LoopbackHandler> = {
            let ivars = HandlerIvars {
                producer: Mutex::new(producer),
                transport: Arc::clone(&transport),
                failure: Arc::clone(&failure),
            };
            let this = LoopbackHandler::alloc().set_ivars(ivars);
            unsafe { objc2::msg_send![super(this), init] }
        };
        let queue = DispatchQueue::new("ai.transcreve.loopback", DispatchQueueAttr::SERIAL);
        let stream = unsafe {
            SCStream::initWithFilter_configuration_delegate(
                SCStream::alloc(),
                &filter,
                &config,
                Some(ProtocolObject::from_ref(&*handler)),
            )
        };
        unsafe {
            stream.addStreamOutput_type_sampleHandlerQueue_error(
                ProtocolObject::from_ref(&*handler),
                SCStreamOutputType::Audio,
                Some(&queue),
            )
        }
        .map_err(|e| {
            format!(
                "Failed to attach the audio output: {}",
                e.localizedDescription()
            )
        })?;

        let (tx, rx) = mpsc::channel();
        let block = RcBlock::new(move |error: *mut NSError| {
            let _ = tx.send(if error.is_null() {
                Ok(())
            } else {
                Err(describe(error))
            });
        });
        unsafe { stream.startCaptureWithCompletionHandler(Some(&block)) };
        rx.recv_timeout(COMPLETION_TIMEOUT)
            .map_err(|_| "Timed out starting ScreenCaptureKit capture".to_string())?
            .map_err(|message| {
                if !CGPreflightScreenCaptureAccess() {
                    permission_denied()
                } else {
                    format!("Failed to start ScreenCaptureKit capture: {message}")
                }
            })?;

        Ok(OpenedLoopback {
            stream: Box::new(SckLoopbackStream {
                stream,
                _handler: handler,
                _queue: queue,
                failure,
                transport,
            }),
            samples: consumer,
        })
    }
}

/// A running SCK capture. Dropping it stops the stream.
struct SckLoopbackStream {
    stream: Retained<SCStream>,
    _handler: Retained<LoopbackHandler>,
    _queue: DispatchRetained<DispatchQueue>,
    failure: Arc<Mutex<Option<String>>>,
    transport: Arc<CaptureTransportState>,
}

impl LoopbackStream for SckLoopbackStream {
    fn device_name(&self) -> &str {
        DEVICE_NAME
    }

    fn sample_rate(&self) -> u32 {
        SAMPLE_RATE
    }

    fn failure(&self) -> Option<String> {
        self.failure.lock().ok().and_then(|slot| slot.clone())
    }

    fn dropped_samples(&self) -> u64 {
        self.transport.overrun_samples.load(Ordering::Relaxed)
    }
}

impl Drop for SckLoopbackStream {
    fn drop(&mut self) {
        unsafe { self.stream.stopCaptureWithCompletionHandler(None) };
    }
}
