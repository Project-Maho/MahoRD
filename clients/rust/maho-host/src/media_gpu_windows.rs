use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, SyncSender};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use super::*;
use crate::capture_windows::CaptureError;

#[path = "capture_windows_gpu.rs"]
mod capture_windows_gpu;
#[path = "encode_windows_hw.rs"]
mod encode_windows_hw;

use capture_windows_gpu::{GpuCapture, GpuFrame};
use encode_windows_hw::{HwMftEncoder, SchedulerAction};

pub(super) fn gpu_source(cpu: WindowsMediaSource) -> Arc<dyn MediaSource> {
    Arc::new(WindowsGpuMediaSource { cpu })
}

struct WindowsGpuMediaSource {
    cpu: WindowsMediaSource,
}

impl MediaSource for WindowsGpuMediaSource {
    fn start(&self, sender: SyncSender<MediaEvent>) -> Result<Box<dyn MediaHandle>, SessionError> {
        let gpu_capture =
            match GpuCapture::new(self.cpu.display_index, self.cpu.width, self.cpu.height) {
                Ok(capture) => capture,
                Err(error) => {
                    crate::host_log(&format!("media pipeline: cpu fallback: dxgi init: {error}"));
                    return self.cpu.start(sender);
                }
            };

        let adapter_name = gpu_capture.adapter_name().to_string();
        let adapter_luid = gpu_capture.adapter_luid();
        let d3d11_device = gpu_capture.d3d11_device();
        let (actual_w, actual_h) = gpu_capture.dimensions();

        let (init_tx, init_rx) = mpsc::sync_channel::<Result<(), String>>(1);

        let mut workers = native_pipeline::Workers::<GpuFrame>::new();
        let first_keyframe_emitted = Arc::new(AtomicBool::new(false));
        let encoder_ready = Arc::new(AtomicBool::new(false));
        let frame_dims = Arc::new(AtomicU64::new(
            (u64::from(actual_w) << 32) | u64::from(actual_h),
        ));

        {
            let sender = sender.clone();
            let first_keyframe_emitted = Arc::clone(&first_keyframe_emitted);
            let encoder_ready = Arc::clone(&encoder_ready);
            let frame_dims = Arc::clone(&frame_dims);
            let display_index = self.cpu.display_index;
            let fps = self.cpu.fps.max(1);
            let origin_x = self.cpu.desktop_x;
            let origin_y = self.cpu.desktop_y;
            let mon_w = actual_w;
            let mon_h = actual_h;

            workers.spawn("maho-win-capture", true, move |handoff| {
                struct KeepAwake;
                impl Drop for KeepAwake {
                    fn drop(&mut self) {
                        display_power::set_keep_awake(false);
                    }
                }
                display_power::set_keep_awake(true);
                let _awake = KeepAwake;

                // Wait until the encoder finishes initialization before starting capture blits.
                while !encoder_ready.load(Ordering::Acquire) {
                    if handoff.is_stopped() {
                        return;
                    }
                    thread::sleep(Duration::from_millis(2));
                }

                let interval = Duration::from_micros(1_000_000 / u64::from(fps));
                let mut capture = gpu_capture;
                let mut last_frame: Option<GpuFrame> = None;
                let mut capture_id = 0_u64;
                let mut consecutive_failures = 0_u32;
                let mut last_emit = Instant::now();
                let mut jiggle_flip = false;
                let mut last_jiggle = Instant::now() - Duration::from_millis(700);

                let send_cursor = |sender: &SyncSender<MediaEvent>,
                                   event: MediaEvent,
                                   frame_id: u64| {
                    let trace = host_trace::enabled();
                    let started = trace.map(|_| Instant::now());
                    if let Some(trace) = trace {
                        trace.record(host_trace::Record {
                            event: 21,
                            frame: frame_id,
                            ..Default::default()
                        });
                    }
                    let res = sender.send(event);
                    if let (Some(trace), Some(started)) = (trace, started) {
                        trace.record(host_trace::Record {
                            event: 22,
                            frame: frame_id,
                            value: u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX),
                            repeat: res.is_err(),
                            ..Default::default()
                        });
                    }
                };

                let query_cursor = || -> (f32, f32, u8) {
                    use windows::Win32::UI::WindowsAndMessaging::{
                        GetCursorInfo, CURSORINFO, CURSOR_SHOWING,
                    };
                    let mut ci = CURSORINFO {
                        cbSize: std::mem::size_of::<CURSORINFO>() as u32,
                        flags: windows::Win32::UI::WindowsAndMessaging::CURSORINFO_FLAGS(0),
                        hCursor: Default::default(),
                        ptScreenPos: Default::default(),
                    };
                    // SAFETY: GetCursorInfo initializes the CURSORINFO struct when cbSize is properly populated.
                    let is_showing = unsafe {
                        GetCursorInfo(&mut ci).is_ok()
                            && (ci.flags.0 & CURSOR_SHOWING.0) == CURSOR_SHOWING.0
                    };
                    if !is_showing {
                        return (-1.0, -1.0, 0);
                    }
                    let px = ci.ptScreenPos.x - origin_x;
                    let py = ci.ptScreenPos.y - origin_y;
                    if px >= 0
                        && px < mon_w as i32
                        && py >= 0
                        && py < mon_h as i32
                        && mon_w > 0
                        && mon_h > 0
                    {
                        let cx = (px as f32) / (mon_w as f32);
                        let cy = (py as f32) / (mon_h as f32);
                        (cx.clamp(0.0, 1.0), cy.clamp(0.0, 1.0), 1)
                    } else {
                        (-1.0, -1.0, 0)
                    }
                };

                while !handoff.is_stopped() {
                    let started = Instant::now();
                    if let Some(trace) = host_trace::enabled() {
                        trace.record(host_trace::Record {
                            event: 9,
                            frame: capture_id + 1,
                            ..Default::default()
                        });
                    }

                    match capture.acquire_and_blt(Duration::from_millis(33), capture_id as usize) {
                        Ok(metadata) => {
                            consecutive_failures = 0;
                            let (w, h) = capture.dimensions();
                            frame_dims.store((u64::from(w) << 32) | u64::from(h), Ordering::Relaxed);

                            if let Some(trace) = host_trace::enabled() {
                                trace.record(host_trace::Record {
                                    event: 10,
                                    frame: capture_id + 1,
                                    value: trace.time(started),
                                    ..Default::default()
                                });
                            }

                            if let Some((px, py)) = metadata
                                .pointer_position
                                .filter(|_| metadata.pointer_visible && w > 0 && h > 0)
                            {
                                let cx = (px as f32) / (w as f32);
                                let cy = (py as f32) / (h as f32);
                                send_cursor(
                                    &sender,
                                    MediaEvent::Cursor(CursorUpdate {
                                        x: cx.clamp(0.0, 1.0),
                                        y: cy.clamp(0.0, 1.0),
                                        cursor_type: 1,
                                    }),
                                    capture_id + 1,
                                );
                            } else {
                                let (cx, cy, ctype) = query_cursor();
                                send_cursor(
                                    &sender,
                                    MediaEvent::Cursor(CursorUpdate {
                                        x: cx,
                                        y: cy,
                                        cursor_type: ctype,
                                    }),
                                    capture_id + 1,
                                );
                            }

                            let captured_at = Instant::now();
                            capture_id += 1;
                            let published_at = Instant::now();
                            let frame = GpuFrame {
                                texture: capture.texture(capture_id as usize),
                                captured_at,
                                capture_id,
                            };
                            last_frame = Some(frame.clone());

                            if let Some(trace) = host_trace::enabled() {
                                trace.record(host_trace::Record {
                                    event: 11,
                                    frame: capture_id,
                                    value: trace.time(captured_at),
                                    ..Default::default()
                                });
                                trace.record(host_trace::Record {
                                    event: 12,
                                    frame: capture_id,
                                    value: trace.time(published_at),
                                    ..Default::default()
                                });
                            }

                            last_emit = started;
                            if !handoff.publish(frame) {
                                break;
                            }
                        }
                        Err(CaptureError::Timeout) => {
                            let (cx, cy, ctype) = query_cursor();
                            send_cursor(
                                &sender,
                                MediaEvent::Cursor(CursorUpdate {
                                    x: cx,
                                    y: cy,
                                    cursor_type: ctype,
                                }),
                                capture_id,
                            );

                            if (!first_keyframe_emitted.load(Ordering::Relaxed)
                                || last_frame.is_none())
                                && last_jiggle.elapsed() >= Duration::from_millis(300)
                            {
                                last_jiggle = started;
                                jiggle_flip = !jiggle_flip;
                                cursor_jiggle::nudge(jiggle_flip);
                            }

                            let keepalive_interval =
                                if first_keyframe_emitted.load(Ordering::Relaxed) {
                                    Duration::from_millis(500)
                                } else {
                                    Duration::from_millis(33)
                                };

                            if last_emit.elapsed() >= keepalive_interval {
                                if let Some(frame) = &last_frame {
                                    last_emit = started;
                                    if let Some(trace) = host_trace::enabled() {
                                        trace.record(host_trace::Record {
                                            event: 13,
                                            frame: frame.capture_id,
                                            value: trace.time(frame.captured_at),
                                            ..Default::default()
                                        });
                                    }
                                    if !handoff.publish(frame.repeated()) {
                                        break;
                                    }
                                }
                            }
                        }
                        Err(error) => {
                            use crate::windows_session::{capture_recovery, CaptureRecovery};

                            let failure = capture_failure_from_label(match &error {
                                CaptureError::AccessLost => "access_lost",
                                CaptureError::AccessDenied => "access_denied",
                                CaptureError::RefreshFailure => "refresh_failure",
                                _ => "capture",
                            });
                            tracing::warn!(%error, consecutive_failures, "GPU DXGI capture interrupted");

                            while !handoff.is_stopped() {
                                match capture_recovery(failure, consecutive_failures) {
                                    CaptureRecovery::Reacquire(delay) => {
                                        thread::sleep(delay);
                                        if handoff.is_stopped() {
                                            break;
                                        }
                                        let rebuilt = GpuCapture::new(
                                            display_index,
                                            mon_w,
                                            mon_h,
                                        );
                                        consecutive_failures =
                                            consecutive_failures.saturating_add(1);
                                        match rebuilt {
                                            Ok(rebuilt) => {
                                                capture = rebuilt;
                                                break;
                                            }
                                            Err(error) => {
                                                tracing::warn!(%error, consecutive_failures, "GPU DXGI reacquire failed");
                                            }
                                        }
                                    }
                                    CaptureRecovery::Abort => {
                                        native_media_error(
                                            &handoff,
                                            &sender,
                                            format!("dxgi gpu: {error}"),
                                        );
                                        return;
                                    }
                                }
                            }
                            continue;
                        }
                    }

                    handoff.pace_until(started + interval);
                }
            })?;
        }

        if self.cpu.capture_audio {
            let sender = sender.clone();
            workers.spawn("maho-win-audio", false, move |handoff| {
                let mut capture = match crate::audio_windows::WindowsAudioCapture::new() {
                    Ok(capture) => capture,
                    Err(error) => {
                        tracing::warn!(%error, "Windows audio loopback unavailable; continuing without audio");
                        return;
                    }
                };
                tracing::info!("Windows audio loopback started");
                while !handoff.is_stopped() {
                    let pcm = capture.poll();
                    if pcm.is_empty() {
                        thread::sleep(Duration::from_millis(5));
                        continue;
                    }
                    let _ = sender.send(MediaEvent::Audio(pcm));
                }
            })?;
        }

        let encoder_config = EncoderConfig {
            width: actual_w,
            height: actual_h,
            bitrate: self.cpu.bitrate,
            fps: self.cpu.fps.max(1),
            keyframe_interval: self.cpu.fps.max(1),
            preferred_codec: VideoCodec::H264,
        };

        {
            let sender = sender.clone();
            let first_keyframe_emitted = Arc::clone(&first_keyframe_emitted);

            workers.spawn("maho-win-encode", true, move |handoff| {
                let mut encoder =
                    match HwMftEncoder::new(&d3d11_device, adapter_luid, encoder_config) {
                        Ok(enc) => {
                            let _ = init_tx.send(Ok(()));
                            enc
                        }
                        Err(error) => {
                            let msg = format!("hwmft init: {error}");
                            let _ = init_tx.send(Err(msg.clone()));
                            native_media_error(&handoff, &sender, msg);
                            return;
                        }
                    };

                let mut pending_frame: Option<GpuFrame> = None;
                let mut encode_started_at = Instant::now();

                while !handoff.is_stopped() {
                    if let Err(error) = encoder.pump_events() {
                        native_media_error(&handoff, &sender, format!("mf pump: {error}"));
                        break;
                    }

                    match encoder.scheduler.decide(pending_frame.is_some()) {
                        SchedulerAction::ProcessOutput => {
                            match encoder.take_output() {
                                Ok(Some(encoded)) => {
                                    let is_key = encoded.is_key_frame;
                                    let trace = host_trace::enabled();
                                    let started = trace.map(|_| Instant::now());
                                    if let Some(trace) = trace {
                                        trace.record(host_trace::Record {
                                            event: 23,
                                            keyframe: is_key,
                                            size: encoded.data.len(),
                                            ..Default::default()
                                        });
                                    }

                                    let video_frame = VideoFrame {
                                        data: encoded.data,
                                        is_key_frame: encoded.is_key_frame,
                                        capture_at: encoded.capture_at,
                                        encode_started_at: encoded.encode_started_at,
                                        encode_completed_at: encoded.encode_completed_at,
                                    };

                                    let sent = sender.send(MediaEvent::Video(video_frame)).is_ok();
                                    if let (Some(trace), Some(started)) = (trace, started) {
                                        trace.record(host_trace::Record {
                                            event: 24,
                                            keyframe: is_key,
                                            value: u64::try_from(started.elapsed().as_micros())
                                                .unwrap_or(u64::MAX),
                                            repeat: !sent,
                                            ..Default::default()
                                        });
                                    }

                                    if sent && is_key {
                                        first_keyframe_emitted.store(true, Ordering::Relaxed);
                                    }
                                    if !sent {
                                        handoff.stop();
                                        break;
                                    }
                                }
                                Ok(None) => {}
                                Err(error) => {
                                    native_media_error(
                                        &handoff,
                                        &sender,
                                        format!("mf output: {error}"),
                                    );
                                    break;
                                }
                            }
                            continue;
                        }
                        SchedulerAction::SubmitInput => {
                            if let Some(frame) = pending_frame.take() {
                                if let Err(error) = encoder.submit_frame(&frame, encode_started_at)
                                {
                                    native_media_error(
                                        &handoff,
                                        &sender,
                                        format!("mf submit: {error}"),
                                    );
                                    break;
                                }
                            }
                            continue;
                        }
                        SchedulerAction::Drained => {
                            break;
                        }
                        SchedulerAction::Wait => {}
                    }

                    if let Some(mut work) = handoff.next_timeout(Duration::from_millis(2)) {
                        handoff.reselect(&mut work);
                        if work.controls.force_keyframe {
                            encoder.force_key_frame();
                        }
                        if let Some(bitrate) = work.controls.bitrate {
                            if let Err(error) = encoder.update_bitrate(bitrate) {
                                native_media_error(
                                    &handoff,
                                    &sender,
                                    format!("mf bitrate: {error}"),
                                );
                            }
                        }
                        if let Some(frame) = work.frame {
                            encode_started_at = Instant::now();
                            pending_frame = Some(frame);
                        }
                    }
                }

                encoder.drain();
                let drain_deadline = Instant::now() + Duration::from_millis(60);
                while Instant::now() < drain_deadline && !encoder.scheduler.drain_complete {
                    let pump_res = encoder.pump_events();
                    if pump_res.is_err() {
                        break;
                    }
                    if encoder.scheduler.has_output {
                        match encoder.take_output() {
                            Ok(Some(encoded)) => {
                                let video_frame = VideoFrame {
                                    data: encoded.data,
                                    is_key_frame: encoded.is_key_frame,
                                    capture_at: encoded.capture_at,
                                    encode_started_at: encoded.encode_started_at,
                                    encode_completed_at: encoded.encode_completed_at,
                                };
                                if sender.send(MediaEvent::Video(video_frame)).is_err() {
                                    break;
                                }
                            }
                            Ok(None) => {}
                            Err(_) => break,
                        }
                    } else {
                        thread::sleep(Duration::from_millis(2));
                    }
                }
            })?;
        }

        workers.handoff.activate();

        match init_rx.recv_timeout(Duration::from_secs(5)) {
            Ok(Ok(())) => {
                encoder_ready.store(true, Ordering::Release);
                crate::host_log(&format!(
                    "media pipeline: gpu d3d11-vp+hwmft h264 {}",
                    adapter_name
                ));
                Ok(Box::new(WindowsGpuMediaHandle { workers }))
            }
            Ok(Err(error)) => {
                workers.stop();
                crate::host_log(&format!("media pipeline: cpu fallback: {error}"));
                self.cpu.start(sender)
            }
            Err(_) => {
                workers.stop();
                crate::host_log("media pipeline: cpu fallback: encoder init timeout");
                self.cpu.start(sender)
            }
        }
    }
}

struct WindowsGpuMediaHandle {
    workers: native_pipeline::Workers<GpuFrame>,
}

impl MediaHandle for WindowsGpuMediaHandle {
    fn force_key_frame(&self) -> Result<(), SessionError> {
        self.workers
            .handoff
            .control(true, None)
            .then_some(())
            .ok_or(SessionError::MediaStopped)
    }

    fn update_bitrate(&self, bitrate: u32) -> Result<(), SessionError> {
        self.workers
            .handoff
            .control(false, Some(bitrate))
            .then_some(())
            .ok_or(SessionError::MediaStopped)
    }

    fn stop(&mut self) {
        self.workers.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fallback_decision_logic() {
        let (init_tx, init_rx) = mpsc::sync_channel::<Result<(), String>>(1);
        init_tx.send(Err("device lost".into())).unwrap();
        let res = init_rx.recv_timeout(Duration::from_millis(50));
        assert!(matches!(res, Ok(Err(msg)) if msg == "device lost"));

        let (_tx2, rx2) = mpsc::sync_channel::<Result<(), String>>(1);
        let res2 = rx2.recv_timeout(Duration::from_millis(10));
        assert!(matches!(res2, Err(mpsc::RecvTimeoutError::Timeout)));
    }

    #[test]
    fn test_worker_activation_before_init_rx_resolves() {
        // Given: workers spawned with native_pipeline::Workers and a fake encoder factory
        let mut workers = native_pipeline::Workers::<u32>::new();
        let (init_tx, init_rx) = mpsc::sync_channel::<Result<(), String>>(1);

        // A fake encoder factory closure that reports Ok upon worker thread entry
        let fake_encoder_factory = move || -> Result<(), String> { Ok(()) };

        workers
            .spawn("test-fake-encoder", true, move |_handoff| {
                let res = fake_encoder_factory();
                let _ = init_tx.send(res);
            })
            .unwrap();

        // When: handoff is activated (matching start() fix)
        workers.handoff.activate();

        // Then: init_rx receives Ok within bounded duration without deadlocking
        let res = init_rx.recv_timeout(Duration::from_secs(1));
        assert!(matches!(res, Ok(Ok(()))));
        workers.stop();
    }
}
