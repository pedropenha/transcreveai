use super::{
    is_microphone_access_denied, is_no_input_device_error, run_consumer, AudioRecorder,
    CaptureProcessor, CaptureTransportState, ChunkDisposition, Cmd, FrameSubscriber, FrameTap,
    VadConfig, VadPolicy,
};
use crate::audio_toolkit::vad::{VadFrame, VoiceActivityDetector};
use rtrb::RingBuffer;
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};

#[test]
fn unopened_recorder_does_not_need_reopen() {
    let recorder = AudioRecorder::new().expect("recorder");
    assert!(!recorder.needs_reopen());
}

#[test]
fn stream_error_requires_reopen() {
    let recorder = AudioRecorder::new().expect("recorder");
    recorder.stream_error.store(true, Ordering::Relaxed);
    assert!(recorder.needs_reopen());
}

/// Pass-through detector with a configurable frame size, standing in for a
/// backend such as Earshot whose frames are not 30 ms.
struct FixedFrameVad(usize);

impl VoiceActivityDetector for FixedFrameVad {
    fn push_frame<'a>(&'a mut self, frame: &'a [f32]) -> anyhow::Result<VadFrame<'a>> {
        Ok(VadFrame::Speech(frame))
    }

    fn frame_samples(&self) -> usize {
        self.0
    }
}

#[test]
fn resampler_frame_size_follows_the_vad_backend() {
    let frame_samples = 256;
    let vad = VadConfig {
        detector: Arc::new(Mutex::new(Box::new(FixedFrameVad(frame_samples)))),
        frame_samples,
        offline_hangover_frames: 0,
        streaming_hangover_frames: 0,
    };
    let frame_lengths = Arc::new(Mutex::new(Vec::new()));
    let observed = Arc::clone(&frame_lengths);
    let mut processor = CaptureProcessor::new(
        16_000,
        Some(vad),
        None,
        vec![FrameSubscriber {
            id: 1,
            tap: FrameTap::Processed,
            when_idle: false,
            callback: Arc::new(move |frame: &[f32]| observed.lock().unwrap().push(frame.len())),
        }],
        Instant::now(),
    )
    .expect("processor");

    let (ready_tx, _ready_rx) = mpsc::channel();
    processor.begin_recording(VadPolicy::Offline, ready_tx);
    processor.process_raw_chunk(&[0.0; 1024], ChunkDisposition::Capture);
    let clip = processor.finish_recording();

    assert_eq!(clip.samples.len(), 1024);
    assert_eq!(clip.captured_samples, 1024);
    // FixedFrameVad does not implement voiced_frames(): it reports 0, and the
    // session ran the VAD so the field is Some rather than None.
    assert_eq!(clip.voiced_samples, Some(0));
    assert_eq!(*frame_lengths.lock().unwrap(), vec![frame_samples; 4]);
}

#[test]
fn idle_chunks_are_discarded_without_reaching_the_recording() {
    let mut processor =
        CaptureProcessor::new(16_000, None, None, Vec::new(), Instant::now()).expect("processor");
    processor.process_raw_chunk(&[1.0; 480], ChunkDisposition::Discard);
    let clip = processor.finish_recording();
    assert!(clip.samples.is_empty());
    // Discarded idle audio never counts toward the session duration either.
    assert_eq!(clip.captured_samples, 0);
}

#[test]
fn monitored_idle_chunks_feed_subscribers_and_the_level_meter() {
    let levels = Arc::new(Mutex::new(Vec::new()));
    let monitored = Arc::new(Mutex::new(Vec::new()));
    let mut processor = CaptureProcessor::new(
        16_000,
        None,
        Some(Arc::new({
            let levels = Arc::clone(&levels);
            move |buckets| levels.lock().unwrap().push(buckets)
        })),
        vec![subscriber(1, FrameTap::Raw, true, Arc::clone(&monitored))],
        Instant::now(),
    )
    .expect("processor");

    // At 16 kHz the visualizer's nearest 30 Hz window is 512 samples.
    processor.process_raw_chunk(&[0.5f32; 512], ChunkDisposition::Monitor);

    // Subscribers consume resampled 30 ms frames; the visualizer still
    // observes the raw 512-sample window needed for its 30 Hz meter.
    assert_eq!(monitored.lock().unwrap().as_slice(), &[0.5f32; 480]);
    assert_eq!(levels.lock().unwrap().len(), 1);
}

#[test]
fn shutdown_is_processed_without_audio_samples() {
    let (_producer, consumer) = RingBuffer::<f32>::new(48_000);
    let (cmd_tx, cmd_rx) = mpsc::channel();
    let (done_tx, done_rx) = mpsc::channel();
    let worker = thread::spawn(move || {
        run_consumer(
            CaptureProcessor::new(48_000, None, None, Vec::new(), Instant::now())
                .expect("processor"),
            consumer,
            cmd_rx,
            Arc::new(CaptureTransportState::default()),
            Arc::new(AtomicBool::new(false)),
        );
        let _ = done_tx.send(());
    });

    cmd_tx.send(Cmd::Shutdown).expect("send shutdown");
    assert!(done_rx.recv_timeout(Duration::from_secs(1)).is_ok());
    worker.join().expect("join consumer");
}

#[test]
fn callback_writes_mono_samples() {
    let (mut producer, mut consumer) = RingBuffer::<f32>::new(8);
    let transport = CaptureTransportState::default();

    AudioRecorder::write_input_to_ring(&[0.25f32, -0.5, 1.0], 1, None, &mut producer, &transport);

    let mut output = [0.0; 3];
    consumer.pop_entire_slice(&mut output).expect("samples");
    assert_eq!(output, [0.25, -0.5, 1.0]);
}

#[test]
fn callback_downmixes_or_selects_multichannel_input() {
    let transport = CaptureTransportState::default();
    let (mut average_tx, mut average_rx) = RingBuffer::<f32>::new(4);
    AudioRecorder::write_input_to_ring(
        &[1.0f32, 3.0, -1.0, 1.0],
        2,
        None,
        &mut average_tx,
        &transport,
    );
    let mut averaged = [0.0; 2];
    average_rx
        .pop_entire_slice(&mut averaged)
        .expect("averaged samples");
    assert_eq!(averaged, [2.0, 0.0]);

    let (mut selected_tx, mut selected_rx) = RingBuffer::<f32>::new(4);
    AudioRecorder::write_input_to_ring(
        &[1.0f32, 3.0, -1.0, 1.0],
        2,
        Some(1),
        &mut selected_tx,
        &transport,
    );
    let mut selected = [0.0; 2];
    selected_rx
        .pop_entire_slice(&mut selected)
        .expect("selected samples");
    assert_eq!(selected, [3.0, 1.0]);
}

#[test]
fn callback_forwards_boundary_block_then_stays_silent_until_resumed() {
    let (mut producer, mut consumer) = RingBuffer::<f32>::new(8);
    let transport = CaptureTransportState::default();

    // The block in hand when a pause is first observed was captured before
    // the stop, so it is forwarded and only then acknowledged.
    transport.pause_requested.store(true, Ordering::Release);
    AudioRecorder::write_input_to_ring(&[1.0f32, 2.0], 1, None, &mut producer, &transport);
    assert!(transport.pause_acknowledged.load(Ordering::Acquire));
    assert_eq!(consumer.slots(), 2);

    // Later blocks while paused are dropped and are not counted as overruns.
    AudioRecorder::write_input_to_ring(&[3.0f32], 1, None, &mut producer, &transport);
    assert_eq!(consumer.slots(), 2);
    assert_eq!(transport.overrun_samples.load(Ordering::Relaxed), 0);

    // Clearing the pause, as the consumer does before stop() returns, resumes capture.
    transport.pause_acknowledged.store(false, Ordering::Relaxed);
    transport.pause_requested.store(false, Ordering::Release);
    AudioRecorder::write_input_to_ring(&[4.0f32], 1, None, &mut producer, &transport);
    let mut output = [0.0; 3];
    consumer.pop_entire_slice(&mut output).expect("samples");
    assert_eq!(output, [1.0, 2.0, 4.0]);
    assert!(!transport.pause_acknowledged.load(Ordering::Acquire));
}

#[test]
fn callback_partially_fills_ring_and_counts_dropped_audio() {
    let (mut producer, mut consumer) = RingBuffer::<f32>::new(2);
    let transport = CaptureTransportState::default();

    AudioRecorder::write_input_to_ring(&[1.0f32, 2.0, 3.0], 1, None, &mut producer, &transport);

    let mut captured = [0.0; 2];
    consumer
        .pop_entire_slice(&mut captured)
        .expect("partial callback audio");
    assert_eq!(captured, [1.0, 2.0]);
    assert_eq!(transport.overrun_samples.load(Ordering::Relaxed), 1);
}

#[test]
fn bounded_drain_leaves_remaining_samples_for_the_next_command_cycle() {
    let (mut producer, mut consumer) = RingBuffer::<f32>::new(8);
    producer
        .push_entire_slice(&[1.0, 2.0, 3.0, 4.0, 5.0])
        .expect("samples");
    let mut drained = Vec::new();

    let count =
        super::drain_available_samples(&mut consumer, 3, |part| drained.extend_from_slice(part));

    assert_eq!(count, 3);
    assert_eq!(drained, [1.0, 2.0, 3.0]);
    assert_eq!(consumer.slots(), 2);
}

#[test]
fn ring_wraparound_preserves_both_read_slices_in_order() {
    let (mut producer, mut consumer) = RingBuffer::<f32>::new(5);
    let transport = CaptureTransportState::default();
    producer
        .push_entire_slice(&[1.0, 2.0, 3.0, 4.0])
        .expect("initial samples");
    let mut discarded = [0.0; 3];
    consumer
        .pop_entire_slice(&mut discarded)
        .expect("advance ring head");

    AudioRecorder::write_input_to_ring(
        &[5.0f32, 6.0, 7.0, 8.0],
        1,
        None,
        &mut producer,
        &transport,
    );

    let chunk = consumer.read_chunk(5).expect("wrapped samples");
    let (first, second) = chunk.as_slices();
    assert!(!first.is_empty());
    assert!(!second.is_empty());
    let ordered = first
        .iter()
        .chain(second.iter())
        .copied()
        .collect::<Vec<_>>();
    assert_eq!(ordered, [4.0, 5.0, 6.0, 7.0, 8.0]);
}

#[test]
fn repeated_start_stop_cycles_resume_capture_without_leaking_samples() {
    let (mut producer, consumer) = RingBuffer::<f32>::new(16_000);
    let transport = Arc::new(CaptureTransportState::default());
    let (cmd_tx, cmd_rx) = mpsc::channel();
    let streamed = Arc::new(Mutex::new(Vec::new()));
    let streamed_cb = Arc::clone(&streamed);
    let consumer_transport = Arc::clone(&transport);
    let worker = thread::spawn(move || {
        let processor = CaptureProcessor::new(
            16_000,
            None,
            None,
            vec![FrameSubscriber {
                id: 1,
                tap: FrameTap::Processed,
                when_idle: false,
                callback: Arc::new(move |frame: &[f32]| {
                    streamed_cb.lock().unwrap().extend_from_slice(frame)
                }),
            }],
            Instant::now(),
        )
        .expect("processor");
        run_consumer(
            processor,
            consumer,
            cmd_rx,
            consumer_transport,
            Arc::new(AtomicBool::new(false)),
        );
    });

    let wait_for_pause_request = || {
        let deadline = Instant::now() + Duration::from_secs(1);
        while !transport.pause_requested.load(Ordering::Acquire) {
            assert!(Instant::now() < deadline, "pause was not requested");
            thread::sleep(Duration::from_millis(1));
        }
    };

    let first_input = [0.25f32, -0.5, 1.0];
    let (ready_tx, ready_rx) = mpsc::channel();
    cmd_tx
        .send(Cmd::Start(VadPolicy::Disabled, Instant::now(), ready_tx))
        .expect("first start");
    AudioRecorder::write_input_to_ring(&first_input, 1, None, &mut producer, &transport);
    ready_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("first capture ready");

    let (reply_tx, reply_rx) = mpsc::channel();
    cmd_tx.send(Cmd::Stop(reply_tx)).expect("first stop");
    wait_for_pause_request();
    // The first callback after Stop carries audio captured before the stop,
    // so it belongs to the recording.
    AudioRecorder::write_input_to_ring(&[99.0f32], 1, None, &mut producer, &transport);

    let first_samples = reply_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("first stop reply")
        .samples;
    let first_expected = [0.25f32, -0.5, 1.0, 99.0];
    assert_eq!(&first_samples[..first_expected.len()], &first_expected);
    assert!(first_samples[first_expected.len()..]
        .iter()
        .all(|&sample| sample == 0.0));
    assert!(!transport.pause_requested.load(Ordering::Acquire));

    let first_streamed_len = {
        let streamed = streamed.lock().unwrap();
        assert_eq!(&streamed[..first_expected.len()], &first_expected);
        streamed.len()
    };

    // Start again immediately after stop() would have returned. The producer
    // must already be re-enabled, and no first-cycle samples may leak through.
    let second_input = [0.75f32, -0.25, 0.5];
    let (ready_tx, ready_rx) = mpsc::channel();
    cmd_tx
        .send(Cmd::Start(VadPolicy::Disabled, Instant::now(), ready_tx))
        .expect("second start");
    AudioRecorder::write_input_to_ring(&second_input, 1, None, &mut producer, &transport);
    ready_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("second capture ready");

    let (reply_tx, reply_rx) = mpsc::channel();
    cmd_tx.send(Cmd::Stop(reply_tx)).expect("second stop");
    wait_for_pause_request();
    AudioRecorder::write_input_to_ring(&[199.0f32], 1, None, &mut producer, &transport);

    let second_samples = reply_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("second stop reply")
        .samples;
    let second_expected = [0.75f32, -0.25, 0.5, 199.0];
    assert_eq!(&second_samples[..second_expected.len()], &second_expected);
    assert!(second_samples[second_expected.len()..]
        .iter()
        .all(|&sample| sample == 0.0));
    assert!(!first_samples
        .iter()
        .any(|sample| second_expected.contains(sample)));
    assert!(!second_samples
        .iter()
        .any(|sample| first_expected.contains(sample)));
    assert!(!transport.pause_requested.load(Ordering::Acquire));

    {
        let streamed = streamed.lock().unwrap();
        assert_eq!(streamed.len(), first_streamed_len + second_samples.len());
        assert_eq!(
            &streamed[first_streamed_len..first_streamed_len + second_expected.len()],
            &second_expected
        );
    }

    cmd_tx.send(Cmd::Shutdown).expect("shutdown");
    worker.join().expect("consumer worker");
}

#[test]
fn missing_callback_at_stop_marks_stream_for_rebuild_and_returns_samples() {
    let (_producer, consumer) = RingBuffer::<f32>::new(16_000);
    let transport = Arc::new(CaptureTransportState::default());
    let stream_error = Arc::new(AtomicBool::new(false));
    let observed_error = Arc::clone(&stream_error);
    let (cmd_tx, cmd_rx) = mpsc::channel();
    let worker_transport = Arc::clone(&transport);
    let worker = thread::spawn(move || {
        run_consumer(
            CaptureProcessor::new(16_000, None, None, Vec::new(), Instant::now())
                .expect("processor"),
            consumer,
            cmd_rx,
            worker_transport,
            stream_error,
        );
    });

    let (ready_tx, _ready_rx) = mpsc::channel();
    cmd_tx
        .send(Cmd::Start(VadPolicy::Disabled, Instant::now(), ready_tx))
        .expect("start");
    let (reply_tx, reply_rx) = mpsc::channel();
    cmd_tx.send(Cmd::Stop(reply_tx)).expect("stop");

    let clip = reply_rx
        .recv_timeout(Duration::from_secs(3))
        .expect("pause timeout still returns captured samples");
    assert!(clip.samples.is_empty());
    worker.join().expect("consumer exits after pause timeout");
    assert!(observed_error.load(Ordering::Acquire));
}

/// VAD that withholds every frame, to prove raw taps see audio that the
/// post-VAD path gates out of a dictation session.
struct SwallowVad;

impl VoiceActivityDetector for SwallowVad {
    fn push_frame<'a>(&'a mut self, _frame: &'a [f32]) -> anyhow::Result<VadFrame<'a>> {
        Ok(VadFrame::Noise)
    }

    fn frame_samples(&self) -> usize {
        480
    }
}

fn subscriber(
    id: u64,
    tap: FrameTap,
    when_idle: bool,
    sink: Arc<Mutex<Vec<f32>>>,
) -> FrameSubscriber {
    FrameSubscriber {
        id,
        tap,
        when_idle,
        callback: Arc::new(move |frame: &[f32]| sink.lock().unwrap().extend_from_slice(frame)),
    }
}

#[test]
fn fan_out_delivers_each_frame_to_every_subscriber() {
    let raw_a = Arc::new(Mutex::new(Vec::new()));
    let raw_b = Arc::new(Mutex::new(Vec::new()));
    let processed = Arc::new(Mutex::new(Vec::new()));
    let mut processor = CaptureProcessor::new(
        16_000,
        None,
        None,
        vec![
            subscriber(1, FrameTap::Raw, false, Arc::clone(&raw_a)),
            subscriber(2, FrameTap::Raw, false, Arc::clone(&raw_b)),
            subscriber(3, FrameTap::Processed, false, Arc::clone(&processed)),
        ],
        Instant::now(),
    )
    .expect("processor");

    let (ready_tx, _ready_rx) = mpsc::channel();
    processor.begin_recording(VadPolicy::Disabled, ready_tx);
    processor.process_raw_chunk(&[0.5f32; 480], ChunkDisposition::Capture);

    // A single stream feeds all consumers: the raw taps and every processed
    // tap each received the whole frame.
    assert_eq!(raw_a.lock().unwrap().as_slice(), &[0.5f32; 480]);
    assert_eq!(raw_b.lock().unwrap().as_slice(), &[0.5f32; 480]);
    assert_eq!(processed.lock().unwrap().as_slice(), &[0.5f32; 480]);
}

#[test]
fn raw_tap_sees_frames_the_vad_withholds() {
    let raw = Arc::new(Mutex::new(Vec::new()));
    let processed = Arc::new(Mutex::new(Vec::new()));
    let vad = VadConfig {
        detector: Arc::new(Mutex::new(Box::new(SwallowVad))),
        frame_samples: 480,
        offline_hangover_frames: 0,
        streaming_hangover_frames: 0,
    };
    let mut processor = CaptureProcessor::new(
        16_000,
        Some(vad),
        None,
        vec![
            subscriber(1, FrameTap::Raw, false, Arc::clone(&raw)),
            subscriber(2, FrameTap::Processed, false, Arc::clone(&processed)),
        ],
        Instant::now(),
    )
    .expect("processor");

    let (ready_tx, _ready_rx) = mpsc::channel();
    processor.begin_recording(VadPolicy::Offline, ready_tx);
    processor.process_raw_chunk(&[0.5f32; 480], ChunkDisposition::Capture);
    let clip = processor.finish_recording();

    assert_eq!(raw.lock().unwrap().as_slice(), &[0.5f32; 480]);
    assert!(processed.lock().unwrap().is_empty());
    assert!(clip.samples.is_empty());
    // The VAD ran and heard nothing: voiced evidence is present and zero —
    // exactly the FR-002-14 "nada ouvido" signal the session layer reads.
    assert_eq!(clip.voiced_samples, Some(0));
    assert_eq!(clip.captured_samples, 480);
}

/// Detector that calls frames with audible energy voiced, letting tests check
/// the "nada ouvido" evidence (`RecordedClip::voiced_samples`) end to end.
struct LoudnessVad {
    voiced_frames: usize,
}

impl LoudnessVad {
    fn new() -> Self {
        Self { voiced_frames: 0 }
    }
}

impl VoiceActivityDetector for LoudnessVad {
    fn push_frame<'a>(&'a mut self, frame: &'a [f32]) -> anyhow::Result<VadFrame<'a>> {
        if frame.iter().any(|sample| sample.abs() > 0.4) {
            self.voiced_frames += 1;
            Ok(VadFrame::Speech(frame))
        } else {
            Ok(VadFrame::Noise)
        }
    }

    fn frame_samples(&self) -> usize {
        480
    }

    fn voiced_frames(&self) -> usize {
        self.voiced_frames
    }
}

fn vad_config(detector: impl VoiceActivityDetector + 'static) -> VadConfig {
    VadConfig {
        detector: Arc::new(Mutex::new(Box::new(detector))),
        frame_samples: 480,
        offline_hangover_frames: 0,
        streaming_hangover_frames: 0,
    }
}

#[test]
fn recorded_clip_reports_voiced_and_captured_samples() {
    let mut processor = CaptureProcessor::new(
        16_000,
        Some(vad_config(LoudnessVad::new())),
        None,
        Vec::new(),
        Instant::now(),
    )
    .expect("processor");

    let (ready_tx, _ready_rx) = mpsc::channel();
    processor.begin_recording(VadPolicy::Offline, ready_tx);
    // One loud (voiced) frame and one silent frame: 60 ms captured, 30 ms voiced.
    processor.process_raw_chunk(&[0.5f32; 480], ChunkDisposition::Capture);
    processor.process_raw_chunk(&[0.0f32; 480], ChunkDisposition::Capture);
    let clip = processor.finish_recording();

    assert_eq!(clip.captured_samples, 960);
    assert_eq!(clip.voiced_samples, Some(480));
    assert_eq!(
        clip.samples.len(),
        480,
        "only the voiced frame passed the gate"
    );
}

#[test]
fn disabled_policy_reports_no_voiced_measurement() {
    let mut processor = CaptureProcessor::new(
        16_000,
        Some(vad_config(LoudnessVad::new())),
        None,
        Vec::new(),
        Instant::now(),
    )
    .expect("processor");

    let (ready_tx, _ready_rx) = mpsc::channel();
    processor.begin_recording(VadPolicy::Disabled, ready_tx);
    processor.process_raw_chunk(&[0.5f32; 480], ChunkDisposition::Capture);
    let clip = processor.finish_recording();

    // Disabled bypasses the VAD entirely: raw audio, no speech measurement.
    assert_eq!(clip.samples.len(), 480);
    assert_eq!(clip.captured_samples, 480);
    assert_eq!(clip.voiced_samples, None);
}

#[test]
fn monitor_subscriber_receives_frames_while_idle_and_recording_stays_clean() {
    let (mut producer, consumer) = RingBuffer::<f32>::new(16_000);
    let transport = Arc::new(CaptureTransportState::default());
    let (cmd_tx, cmd_rx) = mpsc::channel();
    let monitored = Arc::new(Mutex::new(Vec::new()));
    let monitored_sink = Arc::clone(&monitored);
    let worker_transport = Arc::clone(&transport);
    let worker = thread::spawn(move || {
        let processor = CaptureProcessor::new(
            16_000,
            None,
            None,
            vec![subscriber(1, FrameTap::Raw, true, monitored_sink)],
            Instant::now(),
        )
        .expect("processor");
        run_consumer(
            processor,
            consumer,
            cmd_rx,
            worker_transport,
            Arc::new(AtomicBool::new(false)),
        );
    });

    let wait_for_monitored = |min: usize| {
        let deadline = Instant::now() + Duration::from_secs(2);
        while monitored.lock().unwrap().len() < min {
            assert!(
                Instant::now() < deadline,
                "idle frames never reached the monitor subscriber"
            );
            thread::sleep(Duration::from_millis(2));
        }
    };

    // Idle audio flows to the subscriber without any recording session.
    AudioRecorder::write_input_to_ring(&[0.5f32; 480], 1, None, &mut producer, &transport);
    wait_for_monitored(480);

    // A dictation started afterwards must not inherit the monitored audio:
    // begin_recording resets the resampler, so the recording only contains
    // what arrived after Cmd::Start.
    let (ready_tx, ready_rx) = mpsc::channel();
    cmd_tx
        .send(Cmd::Start(VadPolicy::Disabled, Instant::now(), ready_tx))
        .expect("start");
    AudioRecorder::write_input_to_ring(&[0.75f32; 480], 1, None, &mut producer, &transport);
    ready_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("capture ready");

    let (reply_tx, reply_rx) = mpsc::channel();
    cmd_tx.send(Cmd::Stop(reply_tx)).expect("stop");
    let deadline = Instant::now() + Duration::from_secs(1);
    while !transport.pause_requested.load(Ordering::Acquire) {
        assert!(Instant::now() < deadline, "pause was not requested");
        thread::sleep(Duration::from_millis(1));
    }
    AudioRecorder::write_input_to_ring(&[0.9f32], 1, None, &mut producer, &transport);
    let samples = reply_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("stop reply")
        .samples;

    assert!(
        !samples.contains(&0.5),
        "idle monitoring leaked into the recording"
    );
    assert!(samples.contains(&0.75));

    cmd_tx.send(Cmd::Shutdown).expect("shutdown");
    worker.join().expect("consumer worker");
}

#[test]
fn subscribers_attach_and_detach_on_a_live_stream() {
    let (mut producer, consumer) = RingBuffer::<f32>::new(16_000);
    let transport = Arc::new(CaptureTransportState::default());
    let (cmd_tx, cmd_rx) = mpsc::channel();
    let observed = Arc::new(Mutex::new(Vec::new()));
    let worker_transport = Arc::clone(&transport);
    let worker = thread::spawn(move || {
        let processor = CaptureProcessor::new(16_000, None, None, Vec::new(), Instant::now())
            .expect("processor");
        run_consumer(
            processor,
            consumer,
            cmd_rx,
            worker_transport,
            Arc::new(AtomicBool::new(false)),
        );
    });

    // Before subscribing, idle audio is discarded: nothing is observed even
    // after pushing a full frame.
    AudioRecorder::write_input_to_ring(&[0.1f32; 480], 1, None, &mut producer, &transport);
    thread::sleep(Duration::from_millis(50));
    assert!(observed.lock().unwrap().is_empty());

    cmd_tx
        .send(Cmd::AddSubscriber(subscriber(
            7,
            FrameTap::Raw,
            true,
            Arc::clone(&observed),
        )))
        .expect("add subscriber");
    AudioRecorder::write_input_to_ring(&[0.5f32; 480], 1, None, &mut producer, &transport);
    let deadline = Instant::now() + Duration::from_secs(2);
    while observed.lock().unwrap().len() < 480 {
        assert!(
            Instant::now() < deadline,
            "frames never reached the runtime subscriber"
        );
        thread::sleep(Duration::from_millis(2));
    }

    cmd_tx
        .send(Cmd::RemoveSubscriber(7))
        .expect("remove subscriber");
    // Give the consumer a poll cycle to apply the removal, then push again.
    thread::sleep(Duration::from_millis(50));
    let observed_len = observed.lock().unwrap().len();
    AudioRecorder::write_input_to_ring(&[0.25f32; 480], 1, None, &mut producer, &transport);
    thread::sleep(Duration::from_millis(50));
    assert_eq!(
        observed.lock().unwrap().len(),
        observed_len,
        "detached subscriber still received frames"
    );

    cmd_tx.send(Cmd::Shutdown).expect("shutdown");
    worker.join().expect("consumer worker");
}

#[test]
fn subscribe_requires_an_open_stream_and_unsubscribe_clears_the_registry() {
    let recorder = AudioRecorder::new()
        .expect("recorder")
        .with_frame_subscriber(FrameTap::Raw, true, |_| {});
    assert!(recorder
        .subscribe(FrameTap::Raw, true, Arc::new(|_| {}))
        .is_err());
    assert_eq!(
        recorder
            .subscribers
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .len(),
        1
    );
    recorder.unsubscribe(1).expect("remove registry subscriber");
    assert!(recorder
        .subscribers
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .is_empty());
}

#[test]
fn detects_access_is_denied() {
    assert!(is_microphone_access_denied("Access is denied"));
}

#[test]
fn detects_permission_denied() {
    assert!(is_microphone_access_denied("permission denied"));
}

#[test]
fn detects_windows_error_code() {
    assert!(is_microphone_access_denied("WASAPI error: 0x80070005"));
}

#[test]
fn does_not_match_unrelated_errors() {
    assert!(!is_microphone_access_denied("device not found"));
}

#[test]
fn detects_no_input_device() {
    assert!(is_no_input_device_error("No input device found"));
}

#[test]
fn detects_coreaudio_config_error() {
    assert!(is_no_input_device_error(
        "Failed to fetch preferred config: A backend-specific error has occurred: An unknown error unknown to the coreaudio-rs API occurred"
    ));
}

#[test]
fn does_not_match_other_errors_for_no_device() {
    assert!(!is_no_input_device_error("permission denied"));
    assert!(!is_no_input_device_error("device not found"));
}
