use std::{
    fs::File,
    os::fd::{AsRawFd, BorrowedFd, RawFd},
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

use libloading::{Library, Symbol};
use thiserror::Error;
use wayland_client::{
    delegate_noop,
    protocol::{wl_buffer, wl_output, wl_registry},
    Connection, Dispatch, QueueHandle,
};
use wayland_protocols::wp::linux_dmabuf::zv1::client::{
    zwp_linux_buffer_params_v1, zwp_linux_dmabuf_v1,
};
use wayland_protocols_wlr::screencopy::v1::client::{
    zwlr_screencopy_frame_v1, zwlr_screencopy_manager_v1,
};

pub const GBM_BO_USE_RENDERING: u32 = 0x4;
pub const GBM_BO_USE_LINEAR: u32 = 0x10;
pub const GBM_FORMAT_ARGB8888: u32 = 0x34325241;

pub const RING_CAPACITY: usize = 3;

#[derive(Debug, Error)]
pub enum DmabufCaptureError {
    #[error("capture cancelled")]
    Cancelled,
    #[error("capture initialization deadline expired")]
    Timeout,
    #[error("failed to connect to Wayland compositor: {0}")]
    Connect(#[from] wayland_client::ConnectError),
    #[error("Wayland dispatch failed: {0}")]
    Dispatch(#[from] wayland_client::DispatchError),
    #[error("compositor missing required interface: {0}")]
    MissingInterface(&'static str),
    #[error("Wayland output {0:?} was not found")]
    OutputNotFound(Option<String>),
    #[error("screencopy did not offer dmabuf format")]
    NoDmabufOffer,
    #[error("failed to open DRM render node {0}: {1}")]
    DrmOpen(String, std::io::Error),
    #[error("GBM library error: {0}")]
    Gbm(String),
    #[error("screencopy capture failed")]
    CaptureFailed,
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}

struct GbmApi {
    _lib: Library,
    create_device: unsafe extern "C" fn(fd: RawFd) -> *mut std::ffi::c_void,
    destroy_device: unsafe extern "C" fn(device: *mut std::ffi::c_void),
    bo_create: unsafe extern "C" fn(
        device: *mut std::ffi::c_void,
        width: u32,
        height: u32,
        format: u32,
        flags: u32,
    ) -> *mut std::ffi::c_void,
    bo_destroy: unsafe extern "C" fn(bo: *mut std::ffi::c_void),
    bo_get_fd: unsafe extern "C" fn(bo: *mut std::ffi::c_void) -> RawFd,
    bo_get_stride: unsafe extern "C" fn(bo: *mut std::ffi::c_void) -> u32,
    bo_get_offset: unsafe extern "C" fn(bo: *mut std::ffi::c_void, plane: usize) -> u32,
    bo_get_modifier: unsafe extern "C" fn(bo: *mut std::ffi::c_void) -> u64,
}

impl GbmApi {
    fn load() -> Result<Self, DmabufCaptureError> {
        // SAFETY: dlopening libgbm dynamically resolves standard C functions with matching signatures.
        unsafe {
            let lib = Library::new("libgbm.so.1")
                .or_else(|_| Library::new("libgbm.so"))
                .map_err(|e| DmabufCaptureError::Gbm(format!("failed to dlopen libgbm: {e}")))?;

            let create_device: Symbol<unsafe extern "C" fn(RawFd) -> *mut std::ffi::c_void> = lib
                .get(b"gbm_create_device\0")
                .map_err(|e| DmabufCaptureError::Gbm(format!("gbm_create_device: {e}")))?;
            let destroy_device: Symbol<unsafe extern "C" fn(*mut std::ffi::c_void)> = lib
                .get(b"gbm_device_destroy\0")
                .map_err(|e| DmabufCaptureError::Gbm(format!("gbm_device_destroy: {e}")))?;
            let bo_create: Symbol<
                unsafe extern "C" fn(
                    *mut std::ffi::c_void,
                    u32,
                    u32,
                    u32,
                    u32,
                ) -> *mut std::ffi::c_void,
            > = lib
                .get(b"gbm_bo_create\0")
                .map_err(|e| DmabufCaptureError::Gbm(format!("gbm_bo_create: {e}")))?;
            let bo_destroy: Symbol<unsafe extern "C" fn(*mut std::ffi::c_void)> = lib
                .get(b"gbm_bo_destroy\0")
                .map_err(|e| DmabufCaptureError::Gbm(format!("gbm_bo_destroy: {e}")))?;
            let bo_get_fd: Symbol<unsafe extern "C" fn(*mut std::ffi::c_void) -> RawFd> = lib
                .get(b"gbm_bo_get_fd\0")
                .map_err(|e| DmabufCaptureError::Gbm(format!("gbm_bo_get_fd: {e}")))?;
            let bo_get_stride: Symbol<unsafe extern "C" fn(*mut std::ffi::c_void) -> u32> = lib
                .get(b"gbm_bo_get_stride\0")
                .map_err(|e| DmabufCaptureError::Gbm(format!("gbm_bo_get_stride: {e}")))?;
            let bo_get_offset: Symbol<unsafe extern "C" fn(*mut std::ffi::c_void, usize) -> u32> =
                lib.get(b"gbm_bo_get_offset\0")
                    .map_err(|e| DmabufCaptureError::Gbm(format!("gbm_bo_get_offset: {e}")))?;
            let bo_get_modifier: Symbol<unsafe extern "C" fn(*mut std::ffi::c_void) -> u64> = lib
                .get(b"gbm_bo_get_modifier\0")
                .map_err(|e| DmabufCaptureError::Gbm(format!("gbm_bo_get_modifier: {e}")))?;

            Ok(Self {
                create_device: *create_device,
                destroy_device: *destroy_device,
                bo_create: *bo_create,
                bo_destroy: *bo_destroy,
                bo_get_fd: *bo_get_fd,
                bo_get_stride: *bo_get_stride,
                bo_get_offset: *bo_get_offset,
                bo_get_modifier: *bo_get_modifier,
                _lib: lib,
            })
        }
    }
}

pub struct GbmBuffer {
    bo: *mut std::ffi::c_void,
    pub fd: RawFd,
    pub stride: u32,
    pub offset: u32,
    pub wl_buffer: wl_buffer::WlBuffer,
}

pub struct DmabufRingSlot {
    pub gbm: GbmBuffer,
    pub busy: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct DmabufOffer {
    pub format: u32,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone)]
pub struct DmabufCapturedFrame {
    pub ring_index: usize,
    pub captured_at: Instant,
}

struct CaptureState {
    sync_done: bool,
    manager: Option<zwlr_screencopy_manager_v1::ZwlrScreencopyManagerV1>,
    dmabuf: Option<zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1>,
    outputs: Vec<(wl_output::WlOutput, Option<String>)>,
    dmabuf_offer: Option<DmabufOffer>,
    ready: bool,
    failed: bool,
    flags: u32,
}

impl CaptureState {
    fn new() -> Self {
        Self {
            sync_done: false,
            manager: None,
            dmabuf: None,
            outputs: Vec::new(),
            dmabuf_offer: None,
            ready: false,
            failed: false,
            flags: 0,
        }
    }
}

impl Dispatch<wl_registry::WlRegistry, ()> for CaptureState {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_registry::Event::Global {
            name,
            interface,
            version,
        } = event
        {
            if interface == "wl_output" {
                let output =
                    registry.bind::<wl_output::WlOutput, _, _>(name, version.min(4), qh, ());
                state.outputs.push((output, None));
            } else if interface == "zwlr_screencopy_manager_v1" {
                state.manager = Some(
                    registry.bind::<zwlr_screencopy_manager_v1::ZwlrScreencopyManagerV1, _, _>(
                        name,
                        version.min(3),
                        qh,
                        (),
                    ),
                );
            } else if interface == "zwp_linux_dmabuf_v1" {
                state.dmabuf = Some(
                    registry.bind::<zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1, _, _>(
                        name,
                        version.min(3),
                        qh,
                        (),
                    ),
                );
            }
        }
    }
}

impl Dispatch<wl_output::WlOutput, ()> for CaptureState {
    fn event(
        state: &mut Self,
        output: &wl_output::WlOutput,
        event: wl_output::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_output::Event::Name { name } = event {
            for (out, out_name) in &mut state.outputs {
                if out == output {
                    *out_name = Some(name);
                    break;
                }
            }
        }
    }
}

impl Dispatch<zwlr_screencopy_manager_v1::ZwlrScreencopyManagerV1, ()> for CaptureState {
    fn event(
        _: &mut Self,
        _: &zwlr_screencopy_manager_v1::ZwlrScreencopyManagerV1,
        _: zwlr_screencopy_manager_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1, ()> for CaptureState {
    fn event(
        _: &mut Self,
        _: &zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1,
        _: zwp_linux_dmabuf_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<zwp_linux_buffer_params_v1::ZwpLinuxBufferParamsV1, ()> for CaptureState {
    fn event(
        _: &mut Self,
        _: &zwp_linux_buffer_params_v1::ZwpLinuxBufferParamsV1,
        _: zwp_linux_buffer_params_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<zwlr_screencopy_frame_v1::ZwlrScreencopyFrameV1, ()> for CaptureState {
    fn event(
        state: &mut Self,
        _: &zwlr_screencopy_frame_v1::ZwlrScreencopyFrameV1,
        event: zwlr_screencopy_frame_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            zwlr_screencopy_frame_v1::Event::LinuxDmabuf {
                format,
                width,
                height,
            } => {
                if state.dmabuf_offer.is_none() {
                    state.dmabuf_offer = Some(DmabufOffer {
                        format,
                        width,
                        height,
                    });
                }
            }
            zwlr_screencopy_frame_v1::Event::BufferDone => {
                state.sync_done = true;
            }
            zwlr_screencopy_frame_v1::Event::Flags { flags } => {
                state.flags = flags.into();
            }
            zwlr_screencopy_frame_v1::Event::Ready { .. } => {
                state.ready = true;
            }
            zwlr_screencopy_frame_v1::Event::Failed => {
                state.failed = true;
            }
            _ => {}
        }
    }
}

delegate_noop!(CaptureState: ignore wl_buffer::WlBuffer);

pub struct LinuxDmabufCapture {
    _connection: Connection,
    queue: wayland_client::EventQueue<CaptureState>,
    state: CaptureState,
    selected_output: wl_output::WlOutput,
    gbm: GbmApi,
    gbm_device: *mut std::ffi::c_void,
    // Keeps the render-node file descriptor open for the lifetime of gbm_device.
    _drm_file: File,
    pub ring: Vec<DmabufRingSlot>,
    pub offer: DmabufOffer,
    current_slot: usize,
}

impl LinuxDmabufCapture {
    pub fn connect(
        output_name: Option<&str>,
        stop: &AtomicBool,
        deadline: Instant,
    ) -> Result<Self, DmabufCaptureError> {
        let connection = Connection::connect_to_env()?;
        let (mut state, mut queue) = (CaptureState::new(), connection.new_event_queue());
        let qh = queue.handle();
        let _registry = connection.display().get_registry(&qh, ());

        queue.roundtrip(&mut state)?;
        queue.roundtrip(&mut state)?;

        if stop.load(Ordering::Acquire) {
            return Err(DmabufCaptureError::Cancelled);
        }
        if Instant::now() >= deadline {
            return Err(DmabufCaptureError::Timeout);
        }

        let manager = state
            .manager
            .clone()
            .ok_or(DmabufCaptureError::MissingInterface(
                "zwlr_screencopy_manager_v1",
            ))?;
        let dmabuf = state
            .dmabuf
            .clone()
            .ok_or(DmabufCaptureError::MissingInterface("zwp_linux_dmabuf_v1"))?;

        let selected_output = if let Some(target) = output_name {
            state
                .outputs
                .iter()
                .find(|(_, name)| name.as_deref() == Some(target))
                .map(|(out, _)| out.clone())
                .ok_or_else(|| DmabufCaptureError::OutputNotFound(Some(target.to_string())))?
        } else {
            state
                .outputs
                .first()
                .map(|(out, _)| out.clone())
                .ok_or(DmabufCaptureError::OutputNotFound(None))?
        };

        state.sync_done = false;
        state.failed = false;
        let probe_frame = manager.capture_output(1, &selected_output, &qh, ());
        while !state.sync_done && !state.failed {
            if stop.load(Ordering::Acquire) {
                probe_frame.destroy();
                return Err(DmabufCaptureError::Cancelled);
            }
            if Instant::now() >= deadline {
                probe_frame.destroy();
                return Err(DmabufCaptureError::Timeout);
            }
            queue.blocking_dispatch(&mut state)?;
        }
        probe_frame.destroy();

        if state.failed || state.dmabuf_offer.is_none() {
            return Err(DmabufCaptureError::NoDmabufOffer);
        }
        let offer = state.dmabuf_offer.unwrap();

        let drm_path =
            std::env::var("MAHO_VAAPI_DEVICE").unwrap_or_else(|_| "/dev/dri/renderD128".into());
        let drm_file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&drm_path)
            .map_err(|e| DmabufCaptureError::DrmOpen(drm_path.clone(), e))?;

        let gbm = GbmApi::load()?;
        // SAFETY: drm_file has a valid open file descriptor to the DRM render node.
        let gbm_device = unsafe { (gbm.create_device)(drm_file.as_raw_fd()) };
        if gbm_device.is_null() {
            return Err(DmabufCaptureError::Gbm(
                "gbm_create_device returned null".into(),
            ));
        }

        let mut ring = Vec::with_capacity(RING_CAPACITY);
        for _ in 0..RING_CAPACITY {
            // SAFETY: gbm_device is a valid gbm device pointer; format and flags match spec.
            let bo = unsafe {
                (gbm.bo_create)(
                    gbm_device,
                    offer.width,
                    offer.height,
                    offer.format,
                    GBM_BO_USE_RENDERING | GBM_BO_USE_LINEAR,
                )
            };
            if bo.is_null() {
                // SAFETY: gbm_device is a live allocated gbm device.
                unsafe { (gbm.destroy_device)(gbm_device) };
                return Err(DmabufCaptureError::Gbm(
                    "gbm_bo_create returned null".into(),
                ));
            }

            // SAFETY: bo is a valid live gbm buffer object.
            let fd = unsafe { (gbm.bo_get_fd)(bo) };
            // SAFETY: bo is a valid live gbm buffer object.
            let stride = unsafe { (gbm.bo_get_stride)(bo) };
            // SAFETY: bo is a valid live gbm buffer object.
            let offset = unsafe { (gbm.bo_get_offset)(bo, 0) };
            // SAFETY: bo is a valid live gbm buffer object.
            let modifier = unsafe { (gbm.bo_get_modifier)(bo) };

            let params = dmabuf.create_params(&qh, ());
            let mod_hi = (modifier >> 32) as u32;
            let mod_lo = (modifier & 0xFFFF_FFFF) as u32;
            // SAFETY: fd is a valid raw file descriptor returned by gbm_bo_get_fd.
            let borrowed_fd = unsafe { BorrowedFd::borrow_raw(fd) };
            params.add(borrowed_fd, 0, offset, stride, mod_hi, mod_lo);
            let wl_buf = params.create_immed(
                offer.width as i32,
                offer.height as i32,
                offer.format,
                zwp_linux_buffer_params_v1::Flags::empty(),
                &qh,
                (),
            );
            params.destroy();

            ring.push(DmabufRingSlot {
                gbm: GbmBuffer {
                    bo,
                    fd,
                    stride,
                    offset,
                    wl_buffer: wl_buf,
                },
                busy: false,
            });
        }

        queue.roundtrip(&mut state)?;

        Ok(Self {
            _connection: connection,
            queue,
            state,
            selected_output,
            gbm,
            gbm_device,
            _drm_file: drm_file,
            ring,
            offer,
            current_slot: 0,
        })
    }

    pub fn next_free_slot(&mut self) -> Option<usize> {
        for i in 0..RING_CAPACITY {
            let idx = (self.current_slot + i) % RING_CAPACITY;
            if !self.ring[idx].busy {
                self.current_slot = (idx + 1) % RING_CAPACITY;
                return Some(idx);
            }
        }
        None
    }

    pub fn capture_frame(
        &mut self,
        slot_index: usize,
        stop: &AtomicBool,
        timeout: Duration,
    ) -> Result<DmabufCapturedFrame, DmabufCaptureError> {
        let deadline = Instant::now() + timeout;
        let qh = self.queue.handle();
        let manager = self
            .state
            .manager
            .clone()
            .ok_or(DmabufCaptureError::MissingInterface(
                "zwlr_screencopy_manager_v1",
            ))?;

        self.state.sync_done = false;
        self.state.ready = false;
        self.state.failed = false;

        let frame = manager.capture_output(1, &self.selected_output, &qh, ());
        while !self.state.sync_done && !self.state.failed {
            if stop.load(Ordering::Acquire) {
                frame.destroy();
                return Err(DmabufCaptureError::Cancelled);
            }
            if Instant::now() >= deadline {
                frame.destroy();
                return Err(DmabufCaptureError::Timeout);
            }
            self.queue.blocking_dispatch(&mut self.state)?;
        }

        if self.state.failed {
            frame.destroy();
            return Err(DmabufCaptureError::CaptureFailed);
        }

        let wl_buf = &self.ring[slot_index].gbm.wl_buffer;
        frame.copy(wl_buf);

        while !self.state.ready && !self.state.failed {
            if stop.load(Ordering::Acquire) {
                frame.destroy();
                return Err(DmabufCaptureError::Cancelled);
            }
            if Instant::now() >= deadline {
                frame.destroy();
                return Err(DmabufCaptureError::Timeout);
            }
            self.queue.blocking_dispatch(&mut self.state)?;
        }

        frame.destroy();

        if self.state.failed {
            return Err(DmabufCaptureError::CaptureFailed);
        }

        self.ring[slot_index].busy = true;

        Ok(DmabufCapturedFrame {
            ring_index: slot_index,
            captured_at: Instant::now(),
        })
    }

    pub fn release_slot(&mut self, slot_index: usize) {
        if slot_index < self.ring.len() {
            self.ring[slot_index].busy = false;
        }
    }
}

impl Drop for LinuxDmabufCapture {
    fn drop(&mut self) {
        for slot in &mut self.ring {
            slot.gbm.wl_buffer.destroy();
            // SAFETY: slot.gbm.bo and slot.gbm.fd are valid allocated handles being torn down upon drop.
            unsafe {
                (self.gbm.bo_destroy)(slot.gbm.bo);
                libc::close(slot.gbm.fd);
            }
        }
        // SAFETY: self.gbm_device is a live allocated gbm device handle.
        unsafe {
            (self.gbm.destroy_device)(self.gbm_device);
        }
    }
}
