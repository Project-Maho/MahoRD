use super::CaptureError;
use serde::Serialize;
use std::{
    cell::RefCell,
    sync::{mpsc, Mutex},
    thread::{self, JoinHandle},
};
use tauri::{Emitter, Manager, WindowEvent};

type Handle = *mut std::ffi::c_void;
#[repr(C)]
struct KeyData {
    vk: u32,
    scan: u32,
    flags: u32,
    time: u32,
    extra: usize,
}
#[repr(C)]
struct Point {
    x: i32,
    y: i32,
}
#[repr(C)]
struct Message {
    hwnd: Handle,
    message: u32,
    wparam: usize,
    lparam: isize,
    time: u32,
    point: Point,
    private: u32,
}
#[link(name = "user32")]
unsafe extern "system" {
    fn SetWindowsHookExW(
        kind: i32,
        callback: Option<unsafe extern "system" fn(i32, usize, isize) -> isize>,
        module: Handle,
        thread: u32,
    ) -> Handle;
    fn UnhookWindowsHookEx(hook: Handle) -> i32;
    fn CallNextHookEx(hook: Handle, code: i32, wparam: usize, lparam: isize) -> isize;
    fn GetMessageW(message: *mut Message, hwnd: Handle, min: u32, max: u32) -> i32;
    fn PeekMessageW(message: *mut Message, hwnd: Handle, min: u32, max: u32, remove: u32) -> i32;
    fn PostThreadMessageW(thread: u32, message: u32, wparam: usize, lparam: isize) -> i32;
    fn GetForegroundWindow() -> Handle;
}
#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetCurrentThreadId() -> u32;
    fn GetModuleHandleW(name: *const u16) -> Handle;
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct CapturedKey {
    vk_code: u32,
    scan_code: u32,
    extended: bool,
    pressed: bool,
    repeat: bool,
}
struct Context {
    app: tauri::AppHandle,
    hwnd: usize,
    active: bool,
    held: [Option<CapturedKey>; 256],
}
thread_local! { static CONTEXT: RefCell<Option<Context>> = const { RefCell::new(None) }; }
struct Worker {
    id: u32,
    join: JoinHandle<()>,
}
#[derive(Default)]
struct Capture(Mutex<Option<Worker>>);

fn release(context: &mut Context) {
    for key in context.held.iter_mut().filter_map(Option::take) {
        let key = CapturedKey {
            pressed: false,
            repeat: false,
            ..key
        };
        if let Err(error) = context.app.emit("native-key", key) {
            tracing::warn!(%error, "Cannot release captured key");
        }
    }
    if let Err(error) = context.app.emit("native-key-capture-released", ()) {
        tracing::warn!(%error, "Cannot publish capture release");
    }
}

unsafe extern "system" fn callback(code: i32, wparam: usize, lparam: isize) -> isize {
    let handled = std::panic::catch_unwind(|| {
        CONTEXT.with(|slot| {
            let mut slot = slot.borrow_mut();
            let Some(context) = slot.as_mut() else {
                return false;
            };
            if code < 0 {
                return false;
            }
            if !context.active {
                return false;
            }
            // SAFETY: Windows provides a live KBDLLHOOKSTRUCT for nonnegative hook codes.
            let data = unsafe { &*(lparam as *const KeyData) };
            // SAFETY: GetForegroundWindow has no pointer arguments or caller preconditions.
            let focused = unsafe { GetForegroundWindow() } as usize == context.hwnd;
            if !focused {
                context.active = false;
                release(context);
                // SAFETY: this callback runs on the hook thread, whose message queue exists.
                unsafe {
                    PostThreadMessageW(GetCurrentThreadId(), 0x12, 0, 0);
                }
                return false;
            }
            if data.flags & 0x10 != 0 {
                return false;
            }
            let Ok(index) = usize::try_from(data.vk) else {
                return false;
            };
            if index >= context.held.len() {
                return false;
            }
            let pressed = matches!(wparam, 0x100 | 0x104);
            if !pressed && !matches!(wparam, 0x101 | 0x105) {
                return false;
            }
            let control = [0x11, 0xa2, 0xa3]
                .iter()
                .any(|&i| context.held[i].is_some());
            let shift = [0x10, 0xa0, 0xa1]
                .iter()
                .any(|&i| context.held[i].is_some());
            if pressed && data.vk == 0x5a && control && shift {
                context.active = false;
                release(context);
                // SAFETY: called on the hook's message thread; this posts its termination signal.
                unsafe {
                    PostThreadMessageW(GetCurrentThreadId(), 0x12, 0, 0);
                }
                return true;
            }
            let key = CapturedKey {
                vk_code: data.vk,
                scan_code: data.scan,
                extended: data.flags & 1 != 0,
                pressed,
                repeat: pressed && context.held[index].is_some(),
            };
            context.held[index] = if pressed { Some(key.clone()) } else { None };
            if let Err(error) = context.app.emit("native-key", key) {
                tracing::warn!(%error, "Cannot publish captured key");
                context.active = false;
                release(context);
                // SAFETY: this callback runs on the initialized hook thread.
                unsafe {
                    PostThreadMessageW(GetCurrentThreadId(), 0x12, 0, 0);
                }
            }
            true
        })
    });
    match handled {
        Ok(true) => 1,
        Ok(false) | Err(_) => {
            // SAFETY: original Windows callback arguments are passed unchanged to the next hook.
            unsafe { CallNextHookEx(std::ptr::null_mut(), code, wparam, lparam) }
        }
    }
}

pub fn setup(app: &tauri::App) {
    app.manage(Capture::default());
    if let Some(window) = app.get_webview_window("main") {
        let handle = app.handle().clone();
        window.on_window_event(move |event| {
            if matches!(
                event,
                WindowEvent::Focused(false)
                    | WindowEvent::Destroyed
                    | WindowEvent::CloseRequested { .. }
            ) {
                if let Err(error) = stop(&handle) {
                    tracing::warn!(%error, "Cannot stop native capture");
                }
            }
        });
    }
}

pub fn start(window: tauri::WebviewWindow) -> Result<(), CaptureError> {
    stop(window.app_handle())?;
    let hwnd = window
        .hwnd()
        .map_err(|e| CaptureError::Failed(e.to_string()))?
        .0 as usize;
    let app = window.app_handle().clone();
    let capture = app.state::<Capture>();
    let mut worker = capture
        .0
        .lock()
        .map_err(|e| CaptureError::Failed(e.to_string()))?;
    let (tx, rx) = mpsc::sync_channel(1);
    let handle = app.clone();
    let join = thread::Builder::new()
        .name("maho-key-capture".into())
        .spawn(move || {
            // SAFETY: zero is a valid initialization for MSG; PeekMessage creates this thread's queue.
            let mut message: Message = unsafe { std::mem::zeroed() };
            // SAFETY: message points to writable MSG storage; null HWND selects thread messages.
            unsafe {
                PeekMessageW(&mut message, std::ptr::null_mut(), 0, 0, 0);
            }
            CONTEXT.with(|slot| {
                *slot.borrow_mut() = Some(Context {
                    app: handle,
                    hwnd,
                    active: true,
                    held: std::array::from_fn(|_| None),
                })
            });
            // SAFETY: callback has the Win32 hook ABI and stays loaded for the hook lifetime.
            let hook = unsafe {
                SetWindowsHookExW(13, Some(callback), GetModuleHandleW(std::ptr::null()), 0)
            };
            if hook.is_null() {
                let error = std::io::Error::last_os_error();
                let error = if error.raw_os_error() == Some(5) {
                    CaptureError::PermissionDenied(error.to_string())
                } else if error.raw_os_error() == Some(50) {
                    CaptureError::Unsupported(error.to_string())
                } else {
                    CaptureError::Failed(error.to_string())
                };
                if tx.send(Err(error)).is_err() {
                    tracing::warn!("Capture initialization receiver closed");
                }
                CONTEXT.with(|slot| *slot.borrow_mut() = None);
                return;
            }
            // SAFETY: GetCurrentThreadId has no preconditions.
            let id = unsafe { GetCurrentThreadId() };
            if tx.send(Ok(id)).is_ok() {
                // SAFETY: message remains writable and the thread owns this message queue.
                while unsafe { GetMessageW(&mut message, std::ptr::null_mut(), 0, 0) } > 0 {}
            }
            // SAFETY: this thread owns the live hook and unhooks exactly once before context destruction.
            if unsafe { UnhookWindowsHookEx(hook) } == 0 {
                tracing::error!(
                    "Cannot remove native keyboard hook: {}",
                    std::io::Error::last_os_error()
                );
            }
            CONTEXT.with(|slot| {
                if let Some(mut context) = slot.borrow_mut().take() {
                    release(&mut context);
                }
            });
        })
        .map_err(|e| CaptureError::Failed(e.to_string()))?;
    match rx.recv() {
        Ok(Ok(id)) => {
            *worker = Some(Worker { id, join });
            Ok(())
        }
        Ok(Err(error)) => {
            if join.join().is_err() {
                tracing::error!("Capture thread panicked");
            }
            Err(error)
        }
        Err(error) => {
            if join.join().is_err() {
                tracing::error!("Capture thread panicked");
            }
            Err(CaptureError::Failed(error.to_string()))
        }
    }
}

pub fn stop(app: &tauri::AppHandle) -> Result<(), CaptureError> {
    let capture = app.state::<Capture>();
    let mut worker = capture
        .0
        .lock()
        .map_err(|e| CaptureError::Failed(e.to_string()))?;
    if let Some(active) = worker.take() {
        if !active.join.is_finished() {
            // SAFETY: id belongs to the initialized hook thread; WM_QUIT carries no pointers.
            if unsafe { PostThreadMessageW(active.id, 0x12, 0, 0) } == 0 {
                *worker = Some(active);
                return Err(CaptureError::Failed(
                    std::io::Error::last_os_error().to_string(),
                ));
            }
        }
        active
            .join
            .join()
            .map_err(|_| CaptureError::Failed("Capture thread panicked".into()))?;
    }
    Ok(())
}

impl Drop for Capture {
    fn drop(&mut self) {
        if let Ok(worker) = self.0.get_mut() {
            if let Some(active) = worker.take() {
                // SAFETY: the owned hook thread has an initialized queue; WM_QUIT contains no pointers.
                unsafe {
                    PostThreadMessageW(active.id, 0x12, 0, 0);
                }
                if active.join.join().is_err() {
                    tracing::error!("Capture thread panicked during teardown");
                }
            }
        }
    }
}
