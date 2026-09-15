use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};

#[cfg(any(feature = "cpal-output", test))]
use std::sync::mpsc::SyncSender;

#[cfg(feature = "cpal-output")]
use cpal::{
    traits::{DeviceTrait, HostTrait, StreamTrait},
    BufferSize, Device, SampleFormat, SampleRate, Stream, StreamConfig, SupportedStreamConfigRange,
};
use thiserror::Error;

pub const AUDIO_SAMPLE_RATE: u32 = 48_000;
pub const AUDIO_CHANNELS: u16 = 2;
/// 100 ms of interleaved 48 kHz stereo PCM, measured in samples (not frames).
pub const AUDIO_QUEUE_CAPACITY: usize = AUDIO_SAMPLE_RATE as usize * AUDIO_CHANNELS as usize / 10;

/// Shared live PCM queue. Overflow discards only the oldest complete stereo frames.
/// Inputs must be finite and frame-aligned; invalid input never changes the queue.
#[derive(Debug, Clone)]
pub struct AudioQueue {
    state: Arc<Mutex<QueueState>>,
}

#[derive(Debug)]
struct QueueState {
    samples: VecDeque<f32>,
    volume: f32,
    muted: bool,
    underrun_samples: u64,
}

impl Default for AudioQueue {
    fn default() -> Self {
        Self {
            state: Arc::new(Mutex::new(QueueState {
                samples: VecDeque::with_capacity(AUDIO_QUEUE_CAPACITY),
                volume: 1.0,
                muted: false,
                underrun_samples: 0,
            })),
        }
    }
}

impl AudioQueue {
    /// Enqueue little-endian f32 stereo PCM, validating the entire input first.
    pub fn push_pcm_bytes(&self, bytes: &[u8]) -> Result<(), AudioError> {
        if bytes.len() % (std::mem::size_of::<f32>() * AUDIO_CHANNELS as usize) != 0 {
            return Err(AudioError::MisalignedPcm(bytes.len()));
        }
        let samples = bytes
            .chunks_exact(4)
            .map(|sample| f32::from_le_bytes(sample.try_into().expect("four-byte chunk")));
        if samples.clone().any(|sample| !sample.is_finite()) {
            return Err(AudioError::NonFinitePcm);
        }
        // The packet path needs no staging allocation: immutable bytes allow a
        // validation pass followed by copying only the newest bounded tail.
        self.append_validated(samples)
    }

    pub fn push_samples(&self, samples: impl IntoIterator<Item = f32>) -> Result<(), AudioError> {
        // Stage at most 100 ms, outside the callback lock. Every input sample
        // is validated (invalid input never changes the queue), but once
        // staging is full each new sample overwrites the oldest slot in place
        // instead of shifting the deque; a single rotation afterwards restores
        // newest-wins order.
        let mut pending = VecDeque::with_capacity(AUDIO_QUEUE_CAPACITY);
        let mut ring = 0usize;
        let mut overflowed = false;
        let mut input = samples.into_iter();
        while let Some(left) = input.next() {
            let right = input.next().ok_or(AudioError::MisalignedSamples)?;
            if !left.is_finite() || !right.is_finite() {
                return Err(AudioError::NonFinitePcm);
            }
            if pending.len() < AUDIO_QUEUE_CAPACITY {
                pending.push_back(left);
                pending.push_back(right);
            } else {
                pending[ring] = left;
                ring = (ring + 1) % AUDIO_QUEUE_CAPACITY;
                pending[ring] = right;
                ring = (ring + 1) % AUDIO_QUEUE_CAPACITY;
                overflowed = true;
            }
        }
        if overflowed {
            pending.rotate_left(ring);
        }
        self.append_validated(pending.into_iter())
    }

    fn append_validated(
        &self,
        samples: impl ExactSizeIterator<Item = f32>,
    ) -> Result<(), AudioError> {
        let skip = samples.len().saturating_sub(AUDIO_QUEUE_CAPACITY);
        let retained = samples.len() - skip;
        let mut state = self.state.lock().map_err(|_| AudioError::Poisoned)?;
        let overflow = (state.samples.len() + retained).saturating_sub(AUDIO_QUEUE_CAPACITY);
        state.samples.drain(..overflow);
        state.samples.extend(samples.skip(skip));
        Ok(())
    }

    /// Cumulative output slots rendered without queued PCM (underruns).
    pub fn underrun_samples(&self) -> Result<u64, AudioError> {
        Ok(self
            .state
            .lock()
            .map_err(|_| AudioError::Poisoned)?
            .underrun_samples)
    }

    pub fn queued_samples(&self) -> Result<usize, AudioError> {
        Ok(self
            .state
            .lock()
            .map_err(|_| AudioError::Poisoned)?
            .samples
            .len())
    }

    /// Discard buffered PCM across all clones, preserving volume and mute.
    /// Producers must be stopped separately before session teardown.
    pub fn clear(&self) -> Result<(), AudioError> {
        self.state
            .lock()
            .map_err(|_| AudioError::Poisoned)?
            .samples
            .clear();
        Ok(())
    }

    /// Set a finite linear gain in 0..=1. Invalid values leave the gain unchanged.
    pub fn set_volume(&self, volume: f32) -> Result<(), AudioError> {
        if !volume.is_finite() || !(0.0..=1.0).contains(&volume) {
            return Err(AudioError::InvalidVolume(volume));
        }
        self.state.lock().map_err(|_| AudioError::Poisoned)?.volume = volume;
        Ok(())
    }

    pub fn set_muted(&self, muted: bool) -> Result<(), AudioError> {
        self.state.lock().map_err(|_| AudioError::Poisoned)?.muted = muted;
        Ok(())
    }

    /// Fill complete stereo frames, zero-filling underrun. Mute still consumes PCM.
    /// Returns samples consumed; underrun slots are counted in
    /// [`AudioQueue::underrun_samples`]. An odd-sized output is rejected
    /// without mutation.
    pub fn drain_into(&self, output: &mut [f32]) -> Result<usize, AudioError> {
        if output.len() % AUDIO_CHANNELS as usize != 0 {
            return Err(AudioError::MisalignedSamples);
        }
        let mut state = self.state.lock().map_err(|_| AudioError::Poisoned)?;
        let gain = if state.muted { 0.0 } else { state.volume };
        let mut consumed = 0;
        for destination in output {
            if let Some(sample) = state.samples.pop_front() {
                *destination = sample * gain;
                consumed += 1;
            } else {
                state.underrun_samples += 1;
                *destination = 0.0;
            }
        }
        Ok(consumed)
    }
}

#[derive(Debug, Error)]
pub enum AudioError {
    #[error("no default audio output device")]
    NoOutputDevice,
    #[cfg(feature = "cpal-output")]
    #[error("default output uses {0:?}, but the v3 audio path requires f32")]
    UnsupportedSampleFormat(SampleFormat),
    #[error("output device does not support 48 kHz stereo f32 PCM")]
    UnsupportedOutputConfig,
    #[error("PCM byte count {0} is not aligned to stereo f32 frames (8 bytes)")]
    MisalignedPcm(usize),
    #[error("sample count is not aligned to stereo frames")]
    MisalignedSamples,
    #[error("PCM contains a non-finite sample")]
    NonFinitePcm,
    #[error("volume must be finite and in 0..=1, got {0}")]
    InvalidVolume(f32),
    #[error("audio queue lock was poisoned")]
    Poisoned,
    #[error("audio device error: {0}")]
    Device(String),
}

/// An enumerated device handle, not a name-based identifier. Keep this handle for
/// selection: names may collide, and enumeration indices are not persistent IDs.
#[cfg(feature = "cpal-output")]
#[derive(Clone)]
pub struct AudioOutputDevice {
    device: Device,
}

#[cfg(feature = "cpal-output")]
impl AudioOutputDevice {
    pub fn name(&self) -> Result<String, AudioError> {
        self.device
            .name()
            .map_err(|error| AudioError::Device(error.to_string()))
    }

    /// No resampling/channel conversion is performed by this output API.
    pub fn supports_pcm(&self) -> Result<bool, AudioError> {
        Ok(self
            .device
            .supported_output_configs()
            .map_err(|error| AudioError::Device(error.to_string()))?
            .any(supports_pcm_config))
    }
}

#[cfg(feature = "cpal-output")]
fn supports_pcm_config(config: SupportedStreamConfigRange) -> bool {
    config.channels() == AUDIO_CHANNELS
        && config.sample_format() == SampleFormat::F32
        && config.min_sample_rate().0 <= AUDIO_SAMPLE_RATE
        && config.max_sample_rate().0 >= AUDIO_SAMPLE_RATE
}

/// Best-effort notifications on a caller-owned bounded channel. A full or closed
/// channel never blocks playback; cumulative status/errors remain in `status()`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AudioOutputEvent {
    Callback {
        consumed_samples: usize,
        total_consumed_samples: u64,
    },
    Error(String),
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct AudioOutputStatus {
    pub callback_count: u64,
    pub consumed_samples: u64,
    pub underrun_samples: u64,
    pub error_count: u64,
    pub last_error: Option<String>,
}

#[cfg(any(feature = "cpal-output", test))]
#[derive(Default)]
struct CallbackState {
    status: Mutex<AudioOutputStatus>,
    events: Option<SyncSender<AudioOutputEvent>>,
}

#[cfg(any(feature = "cpal-output", test))]
impl CallbackState {
    fn notify(&self, event: AudioOutputEvent) {
        if let Some(events) = &self.events {
            // Lossy by contract: a stalled UI must not stall the audio callback.
            let _ = events.try_send(event);
        }
    }

    fn record_error(&self, error: String) {
        tracing::error!(%error, "CPAL output failed");
        {
            let mut status = self
                .status
                .lock()
                .expect("private callback status lock poisoned");
            status.error_count += 1;
            status.last_error = Some(error.clone());
        }
        self.notify(AudioOutputEvent::Error(error));
    }

    fn render(&self, queue: &AudioQueue, output: &mut [f32]) {
        let consumed = match queue.drain_into(output) {
            Ok(consumed) => consumed,
            Err(error) => {
                output.fill(0.0);
                self.record_error(error.to_string());
                0
            }
        };
        let total_consumed_samples = {
            let mut status = self
                .status
                .lock()
                .expect("private callback status lock poisoned");
            status.callback_count += 1;
            status.consumed_samples += consumed as u64;
            if let Ok(underrun_samples) = queue.underrun_samples() {
                status.underrun_samples = underrun_samples;
            }
            status.consumed_samples
        };
        self.notify(AudioOutputEvent::Callback {
            consumed_samples: consumed,
            total_consumed_samples,
        });
    }
}

/// Active CPAL 48 kHz stereo f32 output stream backed by [`AudioQueue`].
/// Drop stops/releases the stream before clearing buffered PCM. Stop producers
/// first, and use a fresh queue per session; clones remain writable after drop.
#[cfg(feature = "cpal-output")]
pub struct CpalAudioOutput {
    queue: AudioQueue,
    stream: Option<Stream>,
    callbacks: Arc<CallbackState>,
}

#[cfg(feature = "cpal-output")]
impl CpalAudioOutput {
    pub fn output_devices() -> Result<Vec<AudioOutputDevice>, AudioError> {
        Ok(cpal::default_host()
            .output_devices()
            .map_err(|error| AudioError::Device(error.to_string()))?
            .map(|device| AudioOutputDevice { device })
            .collect())
    }

    pub fn default_output_device() -> Result<AudioOutputDevice, AudioError> {
        cpal::default_host()
            .default_output_device()
            .map(|device| AudioOutputDevice { device })
            .ok_or(AudioError::NoOutputDevice)
    }

    /// Compatibility entry point: start the current default output.
    pub fn start(queue: AudioQueue) -> Result<Self, AudioError> {
        Self::start_on_device(queue, None)
    }

    /// `None` selects the current default. An explicit device never silently falls back.
    pub fn start_on_device(
        queue: AudioQueue,
        device: Option<&AudioOutputDevice>,
    ) -> Result<Self, AudioError> {
        Self::start_inner(queue, device, None)
    }

    /// Construct the channel before calling this method so the first callback is
    /// observable. Stream errors are also retained by `status()` if events are lost.
    pub fn start_with_events(
        queue: AudioQueue,
        device: Option<&AudioOutputDevice>,
        events: SyncSender<AudioOutputEvent>,
    ) -> Result<Self, AudioError> {
        Self::start_inner(queue, device, Some(events))
    }

    fn start_inner(
        queue: AudioQueue,
        selected: Option<&AudioOutputDevice>,
        events: Option<SyncSender<AudioOutputEvent>>,
    ) -> Result<Self, AudioError> {
        activate_ios_audio_session()?;
        let device = match selected {
            Some(device) => device.clone(),
            None => Self::default_output_device()?,
        };
        if !device.supports_pcm()? {
            return Err(AudioError::UnsupportedOutputConfig);
        }
        let config = StreamConfig {
            channels: AUDIO_CHANNELS,
            sample_rate: SampleRate(AUDIO_SAMPLE_RATE),
            buffer_size: BufferSize::Default,
        };
        let callback_queue = queue.clone();
        let callbacks = Arc::new(CallbackState {
            status: Mutex::default(),
            events,
        });
        let render_callbacks = callbacks.clone();
        let error_callbacks = callbacks.clone();
        let stream = device
            .device
            .build_output_stream(
                &config,
                move |output: &mut [f32], _| {
                    render_callbacks.render(&callback_queue, output);
                },
                move |error| error_callbacks.record_error(error.to_string()),
                None,
            )
            .map_err(|error| AudioError::Device(error.to_string()))?;
        // Own the stream before play: a start failure follows the same cleanup path.
        let output = Self {
            queue,
            stream: Some(stream),
            callbacks,
        };
        output
            .stream()
            .play()
            .map_err(|error| AudioError::Device(error.to_string()))?;
        Ok(output)
    }

    pub fn queue(&self) -> &AudioQueue {
        &self.queue
    }

    pub fn stream(&self) -> &Stream {
        self.stream
            .as_ref()
            .expect("stream exists until output is dropped")
    }

    pub fn status(&self) -> Result<AudioOutputStatus, AudioError> {
        Ok(self
            .callbacks
            .status
            .lock()
            .map_err(|_| AudioError::Poisoned)?
            .clone())
    }

    pub fn clear(&self) -> Result<(), AudioError> {
        self.queue.clear()
    }
}

#[cfg(feature = "cpal-output")]
impl Drop for CpalAudioOutput {
    fn drop(&mut self) {
        if let Some(stream) = self.stream.take() {
            if let Err(error) = stream.pause() {
                self.callbacks.record_error(error.to_string());
            }
            drop(stream);
        }
        if let Err(error) = self.queue.clear() {
            self.callbacks.record_error(error.to_string());
        }
    }
}

#[cfg(target_os = "ios")]
pub fn activate_ios_audio_session() -> Result<(), AudioError> {
    use std::ffi::{c_char, c_void};

    #[link(name = "objc", kind = "dylib")]
    #[link(name = "AVFoundation", kind = "framework")]
    extern "C" {
        fn objc_getClass(name: *const c_char) -> *mut c_void;
        fn sel_registerName(name: *const c_char) -> *mut c_void;
        fn objc_msgSend();
    }

    unsafe {
        let cls = objc_getClass(b"AVAudioSession\0".as_ptr() as *const c_char);
        if cls.is_null() {
            return Err(AudioError::Device("AVAudioSession class not found".into()));
        }
        let sel_shared = sel_registerName(b"sharedInstance\0".as_ptr() as *const c_char);
        let msg_send_cls: unsafe extern "C" fn(*mut c_void, *mut c_void) -> *mut c_void =
            std::mem::transmute(objc_msgSend as *const ());
        let session = msg_send_cls(cls, sel_shared);
        if session.is_null() {
            return Err(AudioError::Device(
                "AVAudioSession sharedInstance is null".into(),
            ));
        }

        let nsstring_cls = objc_getClass(b"NSString\0".as_ptr() as *const c_char);
        let sel_string_with_utf8 =
            sel_registerName(b"stringWithUTF8String:\0".as_ptr() as *const c_char);
        let msg_send_str: unsafe extern "C" fn(
            *mut c_void,
            *mut c_void,
            *const c_char,
        ) -> *mut c_void = std::mem::transmute(objc_msgSend as *const ());
        let category = msg_send_str(
            nsstring_cls,
            sel_string_with_utf8,
            b"AVAudioSessionCategoryPlayback\0".as_ptr() as *const c_char,
        );

        let sel_set_category = sel_registerName(b"setCategory:error:\0".as_ptr() as *const c_char);
        let mut err: *mut c_void = std::ptr::null_mut();
        let msg_send_set_cat: unsafe extern "C" fn(
            *mut c_void,
            *mut c_void,
            *mut c_void,
            *mut *mut c_void,
        ) -> bool = std::mem::transmute(objc_msgSend as *const ());
        let cat_ok = msg_send_set_cat(session, sel_set_category, category, &mut err);
        if !cat_ok {
            return Err(AudioError::Device(
                "Failed to set AVAudioSessionCategoryPlayback".into(),
            ));
        }

        let sel_set_active = sel_registerName(b"setActive:error:\0".as_ptr() as *const c_char);
        let msg_send_set_active: unsafe extern "C" fn(
            *mut c_void,
            *mut c_void,
            bool,
            *mut *mut c_void,
        ) -> bool = std::mem::transmute(objc_msgSend as *const ());
        let active_ok = msg_send_set_active(session, sel_set_active, true, &mut err);
        if !active_ok {
            return Err(AudioError::Device(
                "Failed to activate AVAudioSession".into(),
            ));
        }
    }
    Ok(())
}

#[cfg(not(target_os = "ios"))]
pub fn activate_ios_audio_session() -> Result<(), AudioError> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::{sync_channel, TryRecvError};

    #[test]
    fn callback_events_report_consumption_and_preserve_errors_when_full() {
        let (sender, receiver) = sync_channel(1);
        let callbacks = CallbackState {
            status: Mutex::default(),
            events: Some(sender),
        };
        let queue = AudioQueue::default();
        queue.push_samples([0.125, -0.25]).unwrap();
        let mut output = [123.0; 4];
        callbacks.render(&queue, &mut output);
        assert_eq!(output, [0.125, -0.25, 0.0, 0.0]);
        callbacks.render(&queue, &mut output); // Full channel must not block.
        callbacks.record_error("test-device-disconnected".into());
        assert_eq!(
            receiver.try_recv().unwrap(),
            AudioOutputEvent::Callback {
                consumed_samples: 2,
                total_consumed_samples: 2,
            }
        );
        assert_eq!(receiver.try_recv(), Err(TryRecvError::Empty));
        let status = callbacks.status.lock().unwrap().clone();
        assert_eq!(status.callback_count, 2);
        assert_eq!(status.consumed_samples, 2);
        assert_eq!(status.error_count, 1);
        assert_eq!(
            status.last_error.as_deref(),
            Some("test-device-disconnected")
        );
        drop(receiver);
        callbacks.render(&queue, &mut output); // Disconnected channel also cannot block.
        assert_eq!(callbacks.status.lock().unwrap().callback_count, 3);
    }

    #[test]
    fn callback_failure_silences_output_and_notifies_error() {
        let (sender, receiver) = sync_channel(2);
        let callbacks = CallbackState {
            status: Mutex::default(),
            events: Some(sender),
        };
        let queue = AudioQueue::default();
        queue.push_samples([0.125, -0.25]).unwrap();
        let mut output = [123.0; 3];
        callbacks.render(&queue, &mut output);
        assert_eq!(output, [0.0; 3]);
        assert_eq!(queue.queued_samples().unwrap(), 2);
        assert!(matches!(
            receiver.try_recv().unwrap(),
            AudioOutputEvent::Error(_)
        ));
        assert_eq!(
            receiver.try_recv().unwrap(),
            AudioOutputEvent::Callback {
                consumed_samples: 0,
                total_consumed_samples: 0,
            }
        );
        assert_eq!(callbacks.status.lock().unwrap().error_count, 1);
    }

    #[cfg(feature = "cpal-output")]
    #[test]
    fn output_configuration_requires_48khz_stereo_f32() {
        let config = |channels, min, max, format| {
            SupportedStreamConfigRange::new(
                channels,
                SampleRate(min),
                SampleRate(max),
                cpal::SupportedBufferSize::Unknown,
                format,
            )
        };
        assert!(supports_pcm_config(config(
            2,
            48_000,
            48_000,
            SampleFormat::F32
        )));
        assert!(supports_pcm_config(config(
            2,
            44_100,
            96_000,
            SampleFormat::F32
        )));
        assert!(!supports_pcm_config(config(
            1,
            48_000,
            48_000,
            SampleFormat::F32
        )));
        assert!(!supports_pcm_config(config(
            2,
            44_100,
            44_100,
            SampleFormat::F32
        )));
        assert!(!supports_pcm_config(config(
            2,
            96_000,
            192_000,
            SampleFormat::F32
        )));
        assert!(!supports_pcm_config(config(
            2,
            48_000,
            48_000,
            SampleFormat::I16
        )));
    }

    #[test]
    fn synthetic_five_second_stream_drains_without_underrun() {
        let queue = AudioQueue::default();
        let sample_count = AUDIO_SAMPLE_RATE as usize * AUDIO_CHANNELS as usize * 5;
        let chunk_samples = 480 * AUDIO_CHANNELS as usize;
        let mut total_consumed = 0;
        for start in (0..sample_count).step_by(chunk_samples) {
            let expected: Vec<_> = (start..start + chunk_samples)
                .map(|sample| (sample as f32 * 0.01).sin())
                .collect();
            queue.push_samples(expected.iter().copied()).unwrap();
            let mut output = vec![0.0; chunk_samples];
            total_consumed += queue.drain_into(&mut output).unwrap();
            assert_eq!(output, expected);
            assert_eq!(queue.queued_samples().unwrap(), 0);
        }
        assert_eq!(total_consumed, sample_count);
    }

    #[test]
    fn underrun_slots_are_counted_in_queue_state_and_status() {
        let queue = AudioQueue::default();
        queue.push_samples([0.125, -0.25]).unwrap();
        let mut output = [0.0; 6];
        assert_eq!(queue.drain_into(&mut output).unwrap(), 2);
        assert_eq!(queue.underrun_samples().unwrap(), 4);

        let (sender, receiver) = sync_channel(1);
        let callbacks = CallbackState {
            status: Mutex::default(),
            events: Some(sender),
        };
        callbacks.render(&queue, &mut output); // All six slots underrun again.
        let status = callbacks.status.lock().unwrap().clone();
        assert_eq!(status.consumed_samples, 0);
        assert_eq!(status.underrun_samples, 10);
        drop(receiver);
    }
}
