use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use thiserror::Error;

use crate::session::DisplayInfo;

#[cfg(any(target_os = "macos", test))]
const CAPTURE_QUEUE_DEPTH: usize = 3;

/// Roughly one second of ScreenCaptureKit's ~20 ms audio packets.
///
/// Audio needs a queue of its own: one 4K keyframe can hold the consumer for
/// several frame intervals, and unlike a dropped frame, a dropped PCM packet is
/// an audible gap. Packets are small (about 7.5 KiB), so the depth is cheap.
#[cfg(any(target_os = "macos", test))]
const AUDIO_QUEUE_DEPTH: usize = 50;

/// The capture callback's publishing end: video and audio are queued
/// independently so neither can evict the other.
#[cfg(any(target_os = "macos", test))]
struct CaptureSenders {
    video: mpsc::SyncSender<CaptureEvent>,
    audio: mpsc::SyncSender<CaptureEvent>,
    dropped_audio: Arc<AtomicU64>,
}

/// Receiving ends of [`CaptureSenders`]. `Stopped` arrives on `video`.
#[cfg(any(target_os = "macos", test))]
pub struct CaptureOutputs {
    pub video: mpsc::Receiver<CaptureEvent>,
    pub audio: mpsc::Receiver<CaptureEvent>,
    dropped_audio: Arc<AtomicU64>,
}

#[cfg(any(target_os = "macos", test))]
impl CaptureOutputs {
    /// Audio packets the capture callback discarded because the consumer
    /// stalled past [`AUDIO_QUEUE_DEPTH`]. Each one is an audible gap.
    pub fn dropped_audio_packets(&self) -> u64 {
        self.dropped_audio.load(Ordering::Relaxed)
    }
}

#[cfg(any(target_os = "macos", test))]
fn capture_channel() -> (CaptureSenders, CaptureOutputs) {
    let (video_tx, video) = mpsc::sync_channel(CAPTURE_QUEUE_DEPTH);
    let (audio_tx, audio) = mpsc::sync_channel(AUDIO_QUEUE_DEPTH);
    let dropped_audio = Arc::new(AtomicU64::new(0));
    (
        CaptureSenders {
            video: video_tx,
            audio: audio_tx,
            dropped_audio: Arc::clone(&dropped_audio),
        },
        CaptureOutputs {
            video,
            audio,
            dropped_audio,
        },
    )
}

#[cfg(any(target_os = "macos", test))]
fn publish_capture(senders: &CaptureSenders, event: CaptureEvent) {
    let audio = matches!(event, CaptureEvent::Audio { .. });
    let sender = if audio {
        &senders.audio
    } else {
        &senders.video
    };
    match sender.try_send(event) {
        Ok(()) => {}
        Err(mpsc::TrySendError::Full(_)) => {
            // A stalled consumer drops the newest event rather than blocking the
            // native callback. Video recovers on the next frame; audio does not,
            // so the loss is counted instead of vanishing silently.
            if audio {
                senders.dropped_audio.fetch_add(1, Ordering::Relaxed);
            }
        }
        Err(mpsc::TrySendError::Disconnected(_)) => {}
    }
}

#[derive(Debug, Clone, Copy)]
pub struct CaptureConfig {
    pub width: u32,
    pub height: u32,
    /// Zero captures at the display's native refresh rate.
    pub frames_per_second: u32,
    pub capture_audio: bool,
}

impl CaptureConfig {
    pub fn native(display: DisplayInfo, capture_audio: bool) -> Self {
        Self {
            width: display.pixel_width,
            height: display.pixel_height,
            frames_per_second: 0,
            capture_audio,
        }
    }
}

#[derive(Debug)]
pub struct CaptureFrame {
    pub width: u32,
    pub height: u32,
    pub bytes_per_row: usize,
    pub bgra: Vec<u8>,
    pub captured_at: Instant,
}

#[derive(Debug)]
pub enum CaptureEvent {
    Video(CaptureFrame),
    /// Wire format: 48 kHz stereo Float32 LE *interleaved* PCM. ScreenCaptureKit
    /// itself delivers non-interleaved (planar) samples, so the capture path
    /// interleaves them before publishing; see [`interleave_planar_f32_stereo`].
    Audio {
        pcm_f32_le: Vec<u8>,
        captured_at: Instant,
    },
    Stopped(String),
}

/// Bytes in one Float32 sample of the wire audio format.
#[cfg(any(target_os = "macos", test))]
const WIRE_AUDIO_SAMPLE_BYTES: usize = 4;

/// Weaves per-channel Float32 LE planes into interleaved stereo, the format
/// every host puts on the wire and `maho-render::audio` plays back.
///
/// Mono duplicates its single plane and channels past the first two are dropped,
/// matching the Windows downmix in `windows_logic::convert_to_wire_audio`.
/// Returns `None` for empty, misaligned, or ragged planes.
#[cfg(any(target_os = "macos", test))]
fn interleave_planar_f32_stereo(planes: &[&[u8]]) -> Option<Vec<u8>> {
    let left = *planes.first()?;
    if left.is_empty() || left.len() % WIRE_AUDIO_SAMPLE_BYTES != 0 {
        return None;
    }
    let right = planes.get(1).copied().unwrap_or(left);
    if right.len() != left.len() {
        return None;
    }
    let mut interleaved = Vec::with_capacity(left.len() * 2);
    for offset in (0..left.len()).step_by(WIRE_AUDIO_SAMPLE_BYTES) {
        interleaved.extend_from_slice(&left[offset..offset + WIRE_AUDIO_SAMPLE_BYTES]);
        interleaved.extend_from_slice(&right[offset..offset + WIRE_AUDIO_SAMPLE_BYTES]);
    }
    Some(interleaved)
}

#[derive(Debug, Error)]
pub enum CaptureError {
    #[error("ScreenCaptureKit is available only on macOS")]
    Unsupported,
    #[error("Screen Recording access is not granted")]
    PermissionDenied,
    #[error("no display is available")]
    NoDisplay,
    #[error("ScreenCaptureKit failed: {0}")]
    ScreenCaptureKit(String),
    #[error("capture setup timed out")]
    Timeout,
    #[error("capture startup cancelled")]
    Cancelled,
}

#[cfg(any(target_os = "macos", test))]
fn check_start(stop: &AtomicBool, deadline: Instant) -> Result<(), CaptureError> {
    if stop.load(Ordering::Relaxed) {
        Err(CaptureError::Cancelled)
    } else if Instant::now() >= deadline {
        Err(CaptureError::Timeout)
    } else {
        Ok(())
    }
}

#[cfg(any(target_os = "macos", test))]
fn wait_for_callback<T>(
    receiver: mpsc::Receiver<Result<T, String>>,
    stop: &AtomicBool,
    deadline: Instant,
) -> Result<T, CaptureError> {
    loop {
        check_start(stop, deadline)?;
        let remaining = deadline.saturating_duration_since(Instant::now());
        match receiver.recv_timeout(remaining.min(Duration::from_millis(50))) {
            Ok(result) => {
                check_start(stop, deadline)?;
                return result.map_err(CaptureError::ScreenCaptureKit);
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Err(CaptureError::ScreenCaptureKit(
                    "callback disconnected".into(),
                ));
            }
        }
    }
}

#[cfg(target_os = "macos")]
mod macos {
    use super::*;
    use block2::RcBlock;
    use core_graphics::display::CGDisplay;
    use objc2::ffi::NSInteger;
    use objc2::rc::Retained;
    use objc2::runtime::{AnyObject, NSObject, NSObjectProtocol, ProtocolObject};
    use objc2::{define_class, msg_send, AnyThread, DeclaredClass};
    use objc2_core_audio_types::{
        kAudioFormatFlagIsFloat, kAudioFormatFlagIsNonInterleaved, AudioBuffer, AudioBufferList,
    };
    use objc2_core_foundation::CFRetained;
    use objc2_core_media::{CMAudioFormatDescriptionGetStreamBasicDescription, CMSampleBuffer};
    use objc2_core_video::{
        kCVPixelFormatType_32BGRA, CVPixelBufferGetBaseAddress, CVPixelBufferGetBytesPerRow,
        CVPixelBufferGetHeight, CVPixelBufferGetPixelFormatType, CVPixelBufferGetWidth,
        CVPixelBufferLockBaseAddress, CVPixelBufferLockFlags, CVPixelBufferUnlockBaseAddress,
    };
    use objc2_foundation::{NSArray, NSError};
    use objc2_screen_capture_kit::{
        SCContentFilter, SCShareableContent, SCStream, SCStreamConfiguration, SCStreamOutput,
        SCStreamOutputType,
    };
    use std::ptr::NonNull;

    use std::sync::Mutex;

    extern "C" {
        fn CGPreflightScreenCaptureAccess() -> bool;
    }

    struct SinkIvars {
        senders: CaptureSenders,
    }

    define_class!(
        #[unsafe(super(NSObject))]
        #[ivars = SinkIvars]
        struct FrameSink;

        unsafe impl NSObjectProtocol for FrameSink {}

        unsafe impl SCStreamOutput for FrameSink {
            #[unsafe(method(stream:didOutputSampleBuffer:ofType:))]
            unsafe fn __stream_did_output_sample_buffer_of_type(
                &self,
                _stream: *mut SCStream,
                sample_buffer: *mut AnyObject,
                output_type: NSInteger,
            ) {
                if sample_buffer.is_null() {
                    return;
                }
                let sample = &*(sample_buffer.cast::<CMSampleBuffer>());
                let event = if output_type == SCStreamOutputType::Screen.0 {
                    copy_video_frame(sample).map(CaptureEvent::Video)
                } else if output_type == SCStreamOutputType::Audio.0 {
                    copy_audio_frame(sample).map(|pcm_f32_le| CaptureEvent::Audio {
                        pcm_f32_le,
                        captured_at: Instant::now(),
                    })
                } else {
                    None
                };
                if let Some(event) = event {
                    publish_capture(&self.ivars().senders, event);
                }
            }
        }
    );

    impl FrameSink {
        fn new(senders: CaptureSenders) -> Retained<Self> {
            let this = Self::alloc().set_ivars(SinkIvars { senders });
            unsafe { msg_send![super(this), init] }
        }
    }

    unsafe fn copy_video_frame(sample: &CMSampleBuffer) -> Option<CaptureFrame> {
        let image = sample.image_buffer()?;
        if CVPixelBufferGetPixelFormatType(&image) != kCVPixelFormatType_32BGRA {
            return None;
        }
        let flags = CVPixelBufferLockFlags::ReadOnly;
        if CVPixelBufferLockBaseAddress(&image, flags) != 0 {
            return None;
        }
        let width = CVPixelBufferGetWidth(&image);
        let height = CVPixelBufferGetHeight(&image);
        let bytes_per_row = CVPixelBufferGetBytesPerRow(&image);
        let base = CVPixelBufferGetBaseAddress(&image).cast::<u8>();
        let bgra = if base.is_null() {
            None
        } else {
            Some(std::slice::from_raw_parts(base, bytes_per_row * height).to_vec())
        };
        let _ = CVPixelBufferUnlockBaseAddress(&image, flags);
        bgra.map(|bgra| CaptureFrame {
            width: width as u32,
            height: height as u32,
            bytes_per_row,
            bgra,
            captured_at: Instant::now(),
        })
    }

    /// Upper bound on the planes read out of one sample buffer. The stream is
    /// configured for stereo; the slack only keeps a surprising layout in-bounds.
    const MAX_AUDIO_PLANES: usize = 8;

    /// `AudioBufferList` is a header plus a trailing array declared as length 1.
    /// This gives the native call room for `MAX_AUDIO_PLANES` buffers.
    #[repr(C)]
    struct AudioBufferListStorage {
        list: AudioBufferList,
        _extra: [AudioBuffer; MAX_AUDIO_PLANES - 1],
    }

    unsafe fn copy_audio_frame(sample: &CMSampleBuffer) -> Option<Vec<u8>> {
        // ScreenCaptureKit hands out non-interleaved Float32: one plane per
        // channel, so the raw block buffer is `[L...][R...]`. Copying it straight
        // through is heard as pitch-shifted static because the wire format is
        // interleaved. Anything else keeps the verbatim copy below.
        if planar_f32_audio(sample) {
            return copy_planar_audio_frame(sample);
        }
        let block = sample.data_buffer()?;
        let length = block.data_length();
        if length == 0 {
            return None;
        }
        let mut bytes = vec![0_u8; length];
        let destination = NonNull::new(bytes.as_mut_ptr().cast()).expect("Vec pointer is non-null");
        if block.copy_data_bytes(0, length, destination) != 0 {
            return None;
        }
        Some(bytes)
    }

    /// True only for the layout [`copy_planar_audio_frame`] knows how to weave:
    /// non-interleaved 32-bit float.
    unsafe fn planar_f32_audio(sample: &CMSampleBuffer) -> bool {
        let Some(description) = sample.format_description() else {
            return false;
        };
        // SAFETY: the description is retained for this scope, and the returned
        // ASBD pointer is read-only storage owned by it.
        let asbd = CMAudioFormatDescriptionGetStreamBasicDescription(&description);
        let Some(asbd) = (unsafe { asbd.as_ref() }) else {
            return false;
        };
        let flags = asbd.mFormatFlags;
        flags & kAudioFormatFlagIsNonInterleaved != 0
            && flags & kAudioFormatFlagIsFloat != 0
            && asbd.mBitsPerChannel as usize == WIRE_AUDIO_SAMPLE_BYTES * 8
    }

    unsafe fn copy_planar_audio_frame(sample: &CMSampleBuffer) -> Option<Vec<u8>> {
        // CoreMedia rejects a buffer list sized for more planes than the sample
        // actually carries, so ask for the exact size before filling it.
        let mut needed = 0_usize;
        // SAFETY: the size out-parameter is a live local, and null buffer-list
        // and block-buffer pointers request a size-only query.
        let status = unsafe {
            sample.audio_buffer_list_with_retained_block_buffer(
                std::ptr::addr_of_mut!(needed),
                std::ptr::null_mut(),
                0,
                None,
                None,
                0,
                std::ptr::null_mut(),
            )
        };
        if status != 0 || needed > std::mem::size_of::<AudioBufferListStorage>() {
            return None;
        }
        // SAFETY: every field is a plain integer or a pointer, for which an
        // all-zero value is valid; the native call fills it before any read.
        let mut storage: AudioBufferListStorage = unsafe { std::mem::zeroed() };
        let mut block: *mut objc2_core_media::CMBlockBuffer = std::ptr::null_mut();
        // SAFETY: the out-parameters are live locals, and `needed` is the size
        // the previous query asked for, bounded by the storage reserved above.
        let status = unsafe {
            sample.audio_buffer_list_with_retained_block_buffer(
                std::ptr::null_mut(),
                std::ptr::addr_of_mut!(storage.list),
                needed,
                None,
                None,
                0,
                std::ptr::addr_of_mut!(block),
            )
        };
        // The returned block buffer owns the plane memory. Adopt it first so it
        // is released on every exit, including the failure path below.
        // SAFETY: the native call returns a +1 reference or null.
        let block = NonNull::new(block).map(|block| unsafe { CFRetained::from_raw(block) });
        if status != 0 {
            return None;
        }
        let block = block?;
        let count = (storage.list.mNumberBuffers as usize).min(MAX_AUDIO_PLANES);
        let mut planes = Vec::with_capacity(count);
        for index in 0..count {
            // SAFETY: `mBuffers` is a trailing array; `count` is clamped to the
            // capacity reserved by `AudioBufferListStorage`.
            let buffer = unsafe { *std::ptr::addr_of!(storage.list.mBuffers[0]).add(index) };
            let data = NonNull::new(buffer.mData)?;
            // SAFETY: the adopted block buffer keeps this plane alive, and the
            // native call reports its length in `mDataByteSize`.
            planes.push(unsafe {
                std::slice::from_raw_parts(
                    data.as_ptr().cast::<u8>(),
                    buffer.mDataByteSize as usize,
                )
            });
        }
        let interleaved = interleave_planar_f32_stereo(&planes);
        drop(block);
        interleaved
    }

    #[allow(clippy::type_complexity)]
    pub struct MacScreenCapture {
        display_info: DisplayInfo,
        stream: Retained<SCStream>,
        _sink: Retained<FrameSink>,
        _queue: dispatch2::DispatchRetained<dispatch2::DispatchQueue>,
        running: bool,
        #[cfg(test)]
        stop_request: Option<Box<dyn Fn(&block2::DynBlock<dyn Fn(*mut NSError)>)>>,
    }

    impl MacScreenCapture {
        pub fn display_info() -> Result<DisplayInfo, CaptureError> {
            let display = CGDisplay::main();
            let logical = display.bounds().size;
            // Secondary displays live at non-zero (possibly negative) origins in
            // CoreGraphics global coordinates; input normalization needs them.
            let origin = display.bounds().origin;
            let pixel_width = display.pixels_wide() as u32;
            let pixel_height = display.pixels_high() as u32;
            if pixel_width == 0
                || pixel_height == 0
                || logical.width <= 0.0
                || logical.height <= 0.0
            {
                return Err(CaptureError::NoDisplay);
            }
            let scale = pixel_width as f64 / logical.width;
            Ok(DisplayInfo {
                desktop_x: origin.x.round() as i32,
                desktop_y: origin.y.round() as i32,
                logical_width: logical.width.round() as u32,
                logical_height: logical.height.round() as u32,
                pixel_width,
                pixel_height,
                scale_factor_milli: (scale * 1_000.0).round() as u32,
            })
        }

        pub fn start(config: CaptureConfig) -> Result<(Self, CaptureOutputs), CaptureError> {
            Self::start_cancellable(
                config,
                &AtomicBool::new(false),
                Instant::now() + Duration::from_secs(15),
            )
        }

        /// Start within one deadline, observing cancellation in 50 ms slices.
        /// Does not request Screen Recording permission.
        pub fn start_cancellable(
            config: CaptureConfig,
            stop: &AtomicBool,
            deadline: Instant,
        ) -> Result<(Self, CaptureOutputs), CaptureError> {
            check_start(stop, deadline)?;
            // SAFETY: CoreGraphics preflight has no arguments or ownership transfer.
            if !unsafe { CGPreflightScreenCaptureAccess() } {
                return Err(CaptureError::PermissionDenied);
            }
            let display_info = Self::display_info()?;

            // SAFETY: SCK callback arguments are borrowed only during invocation;
            // retained values own every object crossing the callback boundary.
            objc2::rc::autoreleasepool(|_| unsafe {
                let (content_tx, content_rx) =
                    mpsc::sync_channel::<Result<Retained<SCShareableContent>, String>>(1);
                let completion = RcBlock::new(
                    move |content: *mut SCShareableContent, error: *mut NSError| {
                        if !error.is_null() {
                            let msg = (*error).localizedDescription().to_string();
                            let _ = content_tx.send(Err(msg));
                        } else if !content.is_null() {
                            let retained = Retained::retain(content)
                                .ok_or_else(|| "null shareable content".into());
                            // Send failure releases the late retained content.
                            let _ = content_tx.send(retained);
                        } else {
                            let _ = content_tx.send(Err("null content and null error".into()));
                        }
                    },
                );
                SCShareableContent::getShareableContentWithCompletionHandler(&completion);
                let content = wait_for_callback(
                    content_rx,
                    stop,
                    deadline.min(Instant::now() + Duration::from_secs(5)),
                )?;
                let displays = content.displays();
                if displays.count() == 0 {
                    return Err(CaptureError::NoDisplay);
                }
                let main_id = CGDisplay::main().id;
                let display = (0..displays.count())
                    .map(|index| displays.objectAtIndex(index))
                    .find(|display| display.displayID() == main_id)
                    .unwrap_or_else(|| displays.objectAtIndex(0));

                let filter = SCContentFilter::initWithDisplay_excludingWindows(
                    SCContentFilter::alloc(),
                    &display,
                    &NSArray::new(),
                );
                let stream_config = SCStreamConfiguration::new();
                stream_config.setWidth(config.width as usize);
                stream_config.setHeight(config.height as usize);
                stream_config.setQueueDepth(3);
                stream_config.setShowsCursor(true);
                stream_config.setPixelFormat(kCVPixelFormatType_32BGRA);
                if config.frames_per_second > 0 {
                    stream_config.setMinimumFrameInterval(objc2_core_media::CMTime::new(
                        1,
                        config.frames_per_second as i32,
                    ));
                } else {
                    stream_config.setMinimumFrameInterval(objc2_core_media::kCMTimeZero);
                }
                stream_config.setCapturesAudio(config.capture_audio);
                // Without this, anything the host itself plays is captured and
                // streamed back to the client, which can feed back into the
                // host's own output.
                stream_config.setExcludesCurrentProcessAudio(true);
                stream_config.setSampleRate(48_000);
                stream_config.setChannelCount(2);

                let stream = SCStream::initWithFilter_configuration_delegate(
                    SCStream::alloc(),
                    &filter,
                    &stream_config,
                    None,
                );
                let (senders, outputs) = capture_channel();
                let sink = FrameSink::new(senders);
                let queue = dispatch2::DispatchQueue::new("maho-host.sck-output", None);
                stream
                    .addStreamOutput_type_sampleHandlerQueue_error(
                        ProtocolObject::from_ref(&*sink),
                        SCStreamOutputType::Screen,
                        Some(&queue),
                    )
                    .map_err(|error| CaptureError::ScreenCaptureKit(error.to_string()))?;
                if config.capture_audio {
                    stream
                        .addStreamOutput_type_sampleHandlerQueue_error(
                            ProtocolObject::from_ref(&*sink),
                            SCStreamOutputType::Audio,
                            Some(&queue),
                        )
                        .map_err(|error| CaptureError::ScreenCaptureKit(error.to_string()))?;
                }

                let (started_tx, started_rx) = mpsc::sync_channel(1);
                let capture = Mutex::new(Some(Self {
                    display_info,
                    stream: stream.clone(),
                    _sink: sink,
                    _queue: queue,
                    running: false,
                    #[cfg(test)]
                    stop_request: None,
                }));
                let started = RcBlock::new(move |error: *mut NSError| {
                    // The copied block owns cleanup until startup completes,
                    // even after the waiting owner cancels or times out.
                    let Some(mut capture) =
                        capture.lock().expect("capture startup poisoned").take()
                    else {
                        return;
                    };
                    let result = if error.is_null() {
                        capture.running = true;
                        Ok(capture)
                    } else {
                        Err((*error).localizedDescription().to_string())
                    };
                    // A late success is dropped and stopped after start completes.
                    let _ = started_tx.send(result);
                });
                check_start(stop, deadline)?;
                stream.startCaptureWithCompletionHandler(Some(&started));
                let capture = wait_for_callback(
                    started_rx,
                    stop,
                    deadline.min(Instant::now() + Duration::from_secs(10)),
                )?;
                Ok((capture, outputs))
            })
        }

        pub fn display(&self) -> DisplayInfo {
            self.display_info
        }

        pub fn stop(self) -> Result<(), CaptureError> {
            self.stop_with_timeout(Duration::from_secs(5))
        }

        fn stop_with_timeout(mut self, timeout: Duration) -> Result<(), CaptureError> {
            let (stopped_tx, stopped_rx) = mpsc::sync_channel(1);
            // The copied block owns teardown resources even if the waiter times out.
            let resources = (self.stream.clone(), self._sink.clone(), self._queue.clone());
            let stopped = RcBlock::new(move |error: *mut NSError| {
                let _ = &resources;
                // SAFETY: SCK lends a valid NSError during this callback, or null.
                let result = unsafe {
                    if error.is_null() {
                        Ok(())
                    } else {
                        Err((*error).localizedDescription().to_string())
                    }
                };
                if let Err(mpsc::SendError(Err(message))) = stopped_tx.send(result) {
                    tracing::warn!(%message, "ScreenCaptureKit late stop failed");
                }
            });
            // Submission transfers cleanup to the completion, including on error.
            // Drop must not submit a second stop after success or waiter timeout.
            self.running = false;
            self.request_stop(&stopped);
            stopped_rx
                .recv_timeout(timeout)
                .map_err(|_| CaptureError::Timeout)?
                .map_err(CaptureError::ScreenCaptureKit)
        }

        fn request_stop(&self, completion: &block2::DynBlock<dyn Fn(*mut NSError)>) {
            #[cfg(test)]
            if let Some(request) = &self.stop_request {
                request(completion);
                return;
            }
            // SAFETY: the stream is retained and SCK copies the escaping block.
            unsafe {
                self.stream
                    .stopCaptureWithCompletionHandler(Some(completion))
            };
        }
    }

    impl Drop for MacScreenCapture {
        fn drop(&mut self) {
            if self.running {
                let resources = (self.stream.clone(), self._sink.clone(), self._queue.clone());
                let stopped = RcBlock::new(move |error: *mut NSError| {
                    let _ = &resources;
                    if !error.is_null() {
                        // SAFETY: SCK lends a valid NSError during this callback.
                        let message = unsafe { (*error).localizedDescription().to_string() };
                        tracing::warn!(%message, "ScreenCaptureKit cleanup failed");
                    }
                });
                self.request_stop(&stopped);
            }
        }
    }

    pub use MacScreenCapture as PlatformScreenCapture;

    #[cfg(test)]
    mod stop_tests {
        use super::*;
        use std::cell::{Cell, RefCell};
        use std::rc::Rc;

        fn capture(
            request: impl Fn(&block2::DynBlock<dyn Fn(*mut NSError)>) + 'static,
        ) -> (MacScreenCapture, objc2::rc::Weak<FrameSink>) {
            // Real retained objects, but no permission query, start, or capture.
            // Only stop submission is scripted at the production FFI seam.
            // SAFETY: all arguments are initialized, retained SCK objects.
            let stream = unsafe {
                SCStream::initWithFilter_configuration_delegate(
                    SCStream::alloc(),
                    &SCContentFilter::new(),
                    &SCStreamConfiguration::new(),
                    None,
                )
            };
            let (senders, _outputs) = capture_channel();
            let sink = FrameSink::new(senders);
            let weak = objc2::rc::Weak::new(&*sink);
            (
                MacScreenCapture {
                    display_info: DisplayInfo {
                        desktop_x: 0,
                        desktop_y: 0,
                        logical_width: 1,
                        logical_height: 1,
                        pixel_width: 1,
                        pixel_height: 1,
                        scale_factor_milli: 1000,
                    },
                    stream,
                    _sink: sink,
                    _queue: dispatch2::DispatchQueue::new("maho-host.stop-test", None),
                    running: true,
                    stop_request: Some(Box::new(request)),
                },
                weak,
            )
        }

        #[test]
        fn explicit_stop_submits_once_when_completion_succeeds() {
            // Given an active capture whose native stop completes immediately.
            let calls = Rc::new(Cell::new(0));
            let observed = Rc::clone(&calls);
            let (capture, _) = capture(move |completion| {
                observed.set(observed.get() + 1);
                completion.call((std::ptr::null_mut(),));
            });
            // When the public consuming stop returns and drops its receiver.
            capture.stop().unwrap();
            // Then Drop does not submit a second native stop.
            assert_eq!(calls.get(), 1, "explicit stop and Drop both submitted stop");
        }

        #[test]
        fn timed_out_stop_retains_resources_until_copied_completion_is_released() {
            // Given a native-style copied callback that has not completed.
            let callbacks = Rc::new(RefCell::new(Vec::new()));
            let copied = Rc::clone(&callbacks);
            let (capture, sink) = capture(move |completion| {
                copied.borrow_mut().push(completion.copy());
            });
            // When the waiter has no remaining budget, return without sleeping.
            assert!(matches!(
                capture.stop_with_timeout(Duration::ZERO),
                Err(CaptureError::Timeout)
            ));
            // Then exactly one copied completion owns the sink after owner drop.
            let mut callbacks = callbacks.borrow_mut();
            assert_eq!(callbacks.len(), 1, "timeout caused a second native stop");
            assert!(
                sink.load().is_some(),
                "resources released before late callback"
            );
            callbacks[0].call((std::ptr::null_mut(),));
            assert!(
                sink.load().is_some(),
                "resources released while callback remains owned"
            );
            callbacks.clear();
            assert!(sink.load().is_none(), "copied completion leaked resources");
        }

        #[test]
        fn explicit_stop_reports_error_without_retrying_in_drop() {
            // Given a stop callback carrying a real NSError, not a null success.
            let calls = Rc::new(Cell::new(0));
            let observed = Rc::clone(&calls);
            let (capture, _) = capture(move |completion| {
                observed.set(observed.get() + 1);
                // SAFETY: valid retained domain; no userInfo ownership transfer.
                let error = unsafe {
                    NSError::initWithDomain_code_userInfo(
                        NSError::alloc(),
                        objc2_foundation::ns_string!("maho.stop-test"),
                        7,
                        None,
                    )
                };
                completion.call((Retained::as_ptr(&error).cast_mut(),));
            });
            // When the consuming public stop observes the callback error.
            let result = capture.stop();
            // Then the typed error escapes and Drop does not retry a native stop.
            assert!(matches!(result, Err(CaptureError::ScreenCaptureKit(_))));
            assert_eq!(calls.get(), 1);
        }

        #[test]
        fn implicit_drop_submits_once_and_retains_late_callback_resources() {
            // Given a running capture whose stop callback is retained by native code.
            let callbacks = Rc::new(RefCell::new(Vec::new()));
            let copied = Rc::clone(&callbacks);
            let (capture, sink) = capture(move |completion| {
                copied.borrow_mut().push(completion.copy());
            });
            // When no explicit stop consumes the capture.
            drop(capture);
            // Then the Drop fallback submits once and owns resources until release.
            let mut callbacks = callbacks.borrow_mut();
            assert_eq!(callbacks.len(), 1);
            assert!(sink.load().is_some());
            callbacks[0].call((std::ptr::null_mut(),));
            callbacks.clear();
            assert!(sink.load().is_none());
        }
    }
}

#[cfg(target_os = "macos")]
pub use macos::PlatformScreenCapture as ScreenCapture;

#[cfg(not(target_os = "macos"))]
pub struct ScreenCapture;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn planar_stereo_planes_are_woven_into_interleaved_frames() {
        // Given: one plane per channel, as ScreenCaptureKit delivers them.
        let left: Vec<u8> = [1.0_f32, 2.0, 3.0]
            .iter()
            .flat_map(|s| s.to_le_bytes())
            .collect();
        let right: Vec<u8> = [-1.0_f32, -2.0, -3.0]
            .iter()
            .flat_map(|s| s.to_le_bytes())
            .collect();
        // When: the capture path converts them to the wire format.
        let wire = interleave_planar_f32_stereo(&[&left, &right]).expect("planes interleave");
        // Then: samples alternate L,R instead of staying plane-ordered.
        let samples: Vec<f32> = wire
            .chunks_exact(4)
            .map(|b| f32::from_le_bytes(b.try_into().unwrap()))
            .collect();
        assert_eq!(samples, vec![1.0, -1.0, 2.0, -2.0, 3.0, -3.0]);
    }

    #[test]
    fn single_plane_audio_is_duplicated_across_both_channels() {
        // Given: a mono capture with only one plane.
        let mono: Vec<u8> = [0.5_f32, -0.25]
            .iter()
            .flat_map(|s| s.to_le_bytes())
            .collect();
        // When / Then: each sample lands in both wire channels.
        let wire = interleave_planar_f32_stereo(&[&mono]).expect("mono interleaves");
        let samples: Vec<f32> = wire
            .chunks_exact(4)
            .map(|b| f32::from_le_bytes(b.try_into().unwrap()))
            .collect();
        assert_eq!(samples, vec![0.5, 0.5, -0.25, -0.25]);
    }

    #[test]
    fn extra_planes_are_dropped_to_the_stereo_wire_format() {
        // Given: a surround layout with more planes than the wire carries.
        let plane = 1.0_f32.to_le_bytes();
        let center = 9.0_f32.to_le_bytes();
        let wire = interleave_planar_f32_stereo(&[&plane, &plane, &center]).expect("interleaves");
        // Then: only the first two channels survive, like the Windows downmix.
        assert_eq!(wire.len(), 8);
        assert!(!wire.windows(4).any(|w| w == center));
    }

    #[test]
    fn unusable_planes_are_rejected_instead_of_emitting_skewed_audio() {
        // Given: no planes, an empty plane, a partial sample, ragged lengths.
        let sample = 1.0_f32.to_le_bytes();
        let ragged: Vec<u8> = [1.0_f32, 2.0]
            .iter()
            .flat_map(|s| s.to_le_bytes())
            .collect();
        // When / Then: every malformed layout is dropped, not reinterpreted.
        assert!(interleave_planar_f32_stereo(&[]).is_none());
        assert!(interleave_planar_f32_stereo(&[&[]]).is_none());
        assert!(interleave_planar_f32_stereo(&[&[0, 1, 2]]).is_none());
        assert!(interleave_planar_f32_stereo(&[&sample, &ragged]).is_none());
    }

    #[test]
    fn interleaved_output_stays_aligned_to_stereo_f32_frames() {
        // Given: a plane sized like one real ScreenCaptureKit callback (960 frames).
        let plane = vec![0_u8; 960 * WIRE_AUDIO_SAMPLE_BYTES];
        let wire = interleave_planar_f32_stereo(&[&plane, &plane]).expect("interleaves");
        // Then: the client's 8-byte stereo frame alignment check passes.
        assert_eq!(wire.len(), 960 * 2 * WIRE_AUDIO_SAMPLE_BYTES);
        assert_eq!(wire.len() % (WIRE_AUDIO_SAMPLE_BYTES * 2), 0);
    }

    #[test]
    fn cancelled_start_drops_late_success_without_waiting_for_callback() {
        // Given: a callback-owned resource and an already cancelled waiter.
        struct CaptureGuard(mpsc::Sender<()>);
        impl Drop for CaptureGuard {
            fn drop(&mut self) {
                self.0.send(()).unwrap();
            }
        }
        let stop = std::sync::atomic::AtomicBool::new(true);
        let (sender, receiver) = mpsc::sync_channel(1);
        let (stopped_tx, stopped_rx) = mpsc::channel();
        // When: cancellation wins, then the native start callback succeeds late.
        let result = wait_for_callback(
            receiver,
            &stop,
            Instant::now() + std::time::Duration::from_secs(5),
        );
        assert!(matches!(result, Err(CaptureError::Cancelled)));
        drop(sender.send(Ok(CaptureGuard(stopped_tx))));
        // Then: the late capture is released rather than orphaned.
        stopped_rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .unwrap();
    }

    #[test]
    fn callback_wait_honors_expired_deadline_with_sender_retained() {
        // Given: a live sender which never calls back and an expired budget.
        let (_sender, receiver) = mpsc::sync_channel::<Result<(), String>>(1);
        let stop = std::sync::atomic::AtomicBool::new(false);
        // When / Then: return Timeout without awaiting a callback.
        assert!(matches!(
            wait_for_callback(receiver, &stop, Instant::now()),
            Err(CaptureError::Timeout)
        ));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn pre_cancelled_start_does_not_request_screen_access() {
        // Given: cancellation before touching native capture/permission APIs.
        let stop = std::sync::atomic::AtomicBool::new(true);
        let config = CaptureConfig {
            width: 1,
            height: 1,
            frames_per_second: 60,
            capture_audio: false,
        };
        // When / Then: typed cancellation, regardless of machine permissions.
        assert!(matches!(
            ScreenCapture::start_cancellable(
                config,
                &stop,
                Instant::now() + std::time::Duration::from_secs(5)
            ),
            Err(CaptureError::Cancelled)
        ));
    }

    #[test]
    fn stalled_capture_consumer_retains_only_queue_capacity() {
        // Given: the callback's actual constructor and publisher, without a consumer.
        let (senders, outputs) = capture_channel();
        // When: alternate raw video/audio events exceed video capacity.
        for index in 0..100 {
            let event = if index % 2 == 0 {
                CaptureEvent::Video(CaptureFrame {
                    width: 1,
                    height: 1,
                    bytes_per_row: 4,
                    bgra: vec![index; 4],
                    captured_at: Instant::now(),
                })
            } else {
                CaptureEvent::Audio {
                    pcm_f32_le: vec![index; 4],
                    captured_at: Instant::now(),
                }
            };
            publish_capture(&senders, event);
        }
        // Then: video keeps only its own capacity, in order.
        let retained: Vec<_> = outputs
            .video
            .try_iter()
            .map(|event| match event {
                CaptureEvent::Video(frame) => frame.bgra[0],
                CaptureEvent::Audio { .. } => panic!("audio evicted a video slot"),
                CaptureEvent::Stopped(_) => panic!("unexpected stop"),
            })
            .collect();
        assert_eq!(
            retained,
            (0..CAPTURE_QUEUE_DEPTH as u8)
                .map(|i| i * 2)
                .collect::<Vec<_>>()
        );
        // And: audio is not evicted by the video burst that overran its queue.
        assert_eq!(outputs.audio.try_iter().count(), 50);
        assert_eq!(outputs.dropped_audio_packets(), 0);
        drop(outputs);
        publish_capture(&senders, CaptureEvent::Stopped(String::new()));
    }

    #[test]
    fn stalled_audio_consumer_drops_are_counted_not_silent() {
        // Given: a stalled consumer and more audio packets than the queue holds.
        let (senders, outputs) = capture_channel();
        let overflow = 7;
        for index in 0..(AUDIO_QUEUE_DEPTH + overflow) {
            publish_capture(
                &senders,
                CaptureEvent::Audio {
                    pcm_f32_le: vec![index as u8; 4],
                    captured_at: Instant::now(),
                },
            );
        }
        // Then: the queue holds its capacity and every lost packet is accounted.
        assert_eq!(outputs.audio.try_iter().count(), AUDIO_QUEUE_DEPTH);
        assert_eq!(outputs.dropped_audio_packets(), overflow as u64);
    }

    #[test]
    fn a_video_burst_never_costs_an_audio_packet() {
        // Given: audio arriving while video overruns its much shallower queue.
        let (senders, outputs) = capture_channel();
        for index in 0..(CAPTURE_QUEUE_DEPTH as u8 + 20) {
            publish_capture(
                &senders,
                CaptureEvent::Video(CaptureFrame {
                    width: 1,
                    height: 1,
                    bytes_per_row: 4,
                    bgra: vec![index; 4],
                    captured_at: Instant::now(),
                }),
            );
        }
        publish_capture(
            &senders,
            CaptureEvent::Audio {
                pcm_f32_le: vec![9; 8],
                captured_at: Instant::now(),
            },
        );
        // Then: the audio packet survives the video backlog.
        assert_eq!(outputs.audio.try_iter().count(), 1);
        assert_eq!(outputs.dropped_audio_packets(), 0);
    }
}

#[cfg(not(target_os = "macos"))]
impl ScreenCapture {
    pub fn display_info() -> Result<DisplayInfo, CaptureError> {
        Err(CaptureError::Unsupported)
    }

    pub fn start(
        _config: CaptureConfig,
    ) -> Result<(Self, mpsc::Receiver<CaptureEvent>), CaptureError> {
        Err(CaptureError::Unsupported)
    }

    pub fn stop(self) -> Result<(), CaptureError> {
        Err(CaptureError::Unsupported)
    }
    pub fn start_cancellable(
        _config: CaptureConfig,
        _stop: &AtomicBool,
        _deadline: Instant,
    ) -> Result<(Self, mpsc::Receiver<CaptureEvent>), CaptureError> {
        Err(CaptureError::Unsupported)
    }
}
