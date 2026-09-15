use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use thiserror::Error;

use crate::session::DisplayInfo;

#[cfg(any(target_os = "macos", test))]
const CAPTURE_QUEUE_DEPTH: usize = 3;

#[cfg(any(target_os = "macos", test))]
fn capture_channel() -> (mpsc::SyncSender<CaptureEvent>, mpsc::Receiver<CaptureEvent>) {
    mpsc::sync_channel(CAPTURE_QUEUE_DEPTH)
}

#[cfg(any(target_os = "macos", test))]
fn publish_capture(sender: &mpsc::SyncSender<CaptureEvent>, event: CaptureEvent) {
    match sender.try_send(event) {
        Ok(()) => {}
        Err(mpsc::TrySendError::Full(_)) => {}
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
    /// ScreenCaptureKit is configured for 48 kHz stereo Float32 interleaved PCM.
    Audio {
        pcm_f32_le: Vec<u8>,
        captured_at: Instant,
    },
    Stopped(String),
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
    use objc2_core_media::CMSampleBuffer;
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
        sender: mpsc::SyncSender<CaptureEvent>,
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
                    publish_capture(&self.ivars().sender, event);
                }
            }
        }
    );

    impl FrameSink {
        fn new(sender: mpsc::SyncSender<CaptureEvent>) -> Retained<Self> {
            let this = Self::alloc().set_ivars(SinkIvars { sender });
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

    unsafe fn copy_audio_frame(sample: &CMSampleBuffer) -> Option<Vec<u8>> {
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

        pub fn start(
            config: CaptureConfig,
        ) -> Result<(Self, mpsc::Receiver<CaptureEvent>), CaptureError> {
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
        ) -> Result<(Self, mpsc::Receiver<CaptureEvent>), CaptureError> {
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
                stream_config.setSampleRate(48_000);
                stream_config.setChannelCount(2);

                let stream = SCStream::initWithFilter_configuration_delegate(
                    SCStream::alloc(),
                    &filter,
                    &stream_config,
                    None,
                );
                let (sender, receiver) = capture_channel();
                let sink = FrameSink::new(sender);
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
                Ok((capture, receiver))
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
            let (sender, _receiver) = capture_channel();
            let sink = FrameSink::new(sender);
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
        let (sender, receiver) = capture_channel();
        // When: alternate raw video/audio events exceed capacity.
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
            publish_capture(&sender, event);
        }
        // Then: only the first capacity events remain, in order.
        let retained: Vec<_> = receiver
            .try_iter()
            .map(|event| match event {
                CaptureEvent::Video(frame) => frame.bgra[0],
                CaptureEvent::Audio { pcm_f32_le, .. } => pcm_f32_le[0],
                CaptureEvent::Stopped(_) => panic!("unexpected stop"),
            })
            .collect();
        assert_eq!(retained, (0..CAPTURE_QUEUE_DEPTH as u8).collect::<Vec<_>>());
        drop(receiver);
        publish_capture(&sender, CaptureEvent::Stopped(String::new()));
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
