//! LocalSystem Windows service that supervises a SYSTEM capture worker.

use std::ffi::{c_void, OsStr};
use std::io;
use std::os::windows::ffi::OsStrExt;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use windows::core::{PCWSTR, PWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::Security::*;
use windows::Win32::System::Environment::*;
use windows::Win32::System::RemoteDesktop::WTSGetActiveConsoleSessionId;
use windows::Win32::System::Services::*;
use windows::Win32::System::StationsAndDesktops::*;
use windows::Win32::System::Threading::*;

use crate::windows_session::{ConsoleState, DesktopKind, SupervisorAction, WorkerState};

pub const SERVICE_NAME: &str = "MahoRDHost";
pub const SERVICE_DISPLAY_NAME: &str = "MahoRD Host";

#[derive(Debug)]
pub struct WorkerHandle {
    pub process_id: u32,
    pub session_id: u32,
    pub desktop: DesktopKind,
}

fn io_error(error: windows::core::Error) -> io::Error {
    let code = error.code().0 as u32;
    if code & 0xffff_0000 == 0x8007_0000 {
        io::Error::from_raw_os_error((code & 0xffff) as i32)
    } else {
        io::Error::other(error)
    }
}

/// Service mode has no console, so diagnostics go to a file the SCM account can
/// always write. Without this the supervision loop fails silently.
pub fn log_to_file() {
    use std::io::Write;

    let path = std::env::var("ProgramData")
        .map(|root| Path::new(&root).join("MahoRD").join("service.log"))
        .unwrap_or_else(|_| Path::new("C:\\ProgramData\\MahoRD\\service.log").to_path_buf());
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        let mut handle = file;
        let _ = writeln!(
            handle,
            "--- service start {:?} ---",
            std::time::SystemTime::now()
        );
        let _ = handle.flush();
        SERVICE_LOG_PATH.set(path).ok();
    }
}

static SERVICE_LOG_PATH: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();

pub(crate) fn service_log(message: &str) {
    use std::io::Write;

    let Some(path) = SERVICE_LOG_PATH.get() else {
        return;
    };
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = writeln!(file, "{message}");
        let _ = file.flush();
    }
}

fn wide(value: &OsStr) -> Vec<u16> {
    value.encode_wide().chain(Some(0)).collect()
}

fn command_line(exe: &Path, argument: &str) -> io::Result<Vec<u16>> {
    let path: Vec<u16> = exe.as_os_str().encode_wide().collect();
    if path.iter().any(|c| *c == 0 || *c == b'"' as u16) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid executable path",
        ));
    }
    let mut command = vec![b'"' as u16];
    command.extend(path);
    command.extend(format!("\" {argument}").encode_utf16());
    command.push(0);
    Ok(command)
}

struct KernelHandle(HANDLE);
impl Drop for KernelHandle {
    fn drop(&mut self) {
        // SAFETY: this wrapper exclusively owns a successfully opened kernel handle.
        if let Err(error) = unsafe { CloseHandle(self.0) } {
            tracing::warn!(%error, "CloseHandle failed");
        }
    }
}

struct ServiceHandle(SC_HANDLE);
impl Drop for ServiceHandle {
    fn drop(&mut self) {
        // SAFETY: this wrapper exclusively owns a successfully opened SCM handle.
        if let Err(error) = unsafe { CloseServiceHandle(self.0) } {
            tracing::warn!(%error, "CloseServiceHandle failed");
        }
    }
}

struct DesktopHandle(HDESK);
impl Drop for DesktopHandle {
    fn drop(&mut self) {
        // SAFETY: this desktop was opened by OpenInputDesktop and is not assigned to a thread.
        if let Err(error) = unsafe { CloseDesktop(self.0) } {
            tracing::warn!(%error, "CloseDesktop failed");
        }
    }
}

struct Environment(*mut c_void);
impl Drop for Environment {
    fn drop(&mut self) {
        // SAFETY: the pointer is the block returned by CreateEnvironmentBlock.
        if let Err(error) = unsafe { DestroyEnvironmentBlock(self.0) } {
            tracing::warn!(%error, "DestroyEnvironmentBlock failed");
        }
    }
}

pub fn install(exe: &Path) -> io::Result<()> {
    let name = wide(OsStr::new(SERVICE_NAME));
    let display = wide(OsStr::new(SERVICE_DISPLAY_NAME));
    let command = command_line(exe, "--service-run")?;
    // SAFETY: strings are NUL-terminated and remain alive for each synchronous SCM call.
    unsafe {
        let manager = ServiceHandle(
            OpenSCManagerW(PCWSTR::null(), PCWSTR::null(), SC_MANAGER_CREATE_SERVICE)
                .map_err(io_error)?,
        );
        let access = SERVICE_CHANGE_CONFIG | SERVICE_START;
        let service = match CreateServiceW(
            manager.0,
            PCWSTR(name.as_ptr()),
            PCWSTR(display.as_ptr()),
            access,
            SERVICE_WIN32_OWN_PROCESS,
            SERVICE_AUTO_START,
            SERVICE_ERROR_NORMAL,
            PCWSTR(command.as_ptr()),
            PCWSTR::null(),
            None,
            PCWSTR::null(),
            PCWSTR::null(),
            PCWSTR::null(),
        ) {
            Ok(handle) => ServiceHandle(handle),
            Err(error) if error.code() == ERROR_SERVICE_EXISTS.to_hresult() => {
                let service = ServiceHandle(
                    OpenServiceW(manager.0, PCWSTR(name.as_ptr()), access).map_err(io_error)?,
                );
                ChangeServiceConfigW(
                    service.0,
                    ENUM_SERVICE_TYPE(SERVICE_NO_CHANGE),
                    SERVICE_START_TYPE(SERVICE_NO_CHANGE),
                    SERVICE_ERROR(SERVICE_NO_CHANGE),
                    PCWSTR(command.as_ptr()),
                    PCWSTR::null(),
                    None,
                    PCWSTR::null(),
                    PCWSTR::null(),
                    PCWSTR::null(),
                    PCWSTR::null(),
                )
                .map_err(io_error)?;
                service
            }
            Err(error) => return Err(io_error(error)),
        };
        apply_failure_actions(service.0);
        match StartServiceW(service.0, None) {
            Ok(()) => Ok(()),
            Err(error) if error.code() == ERROR_SERVICE_ALREADY_RUNNING.to_hresult() => Ok(()),
            Err(error) => Err(io_error(error)),
        }
    }
}

/// Registers the SCM restart policy. Without it the default is "take no
/// action", so a crash leaves the host stopped until someone logs in — the one
/// state this service exists to avoid. A failure to configure recovery is not
/// worth refusing the install over, so it is logged and tolerated.
fn apply_failure_actions(service: SC_HANDLE) {
    let policy = crate::windows_session::service_failure_actions();
    let mut actions: Vec<SC_ACTION> = policy
        .restart_delays_ms
        .iter()
        .map(|delay| SC_ACTION {
            Type: SC_ACTION_RESTART,
            Delay: *delay,
        })
        .collect();
    let failure = SERVICE_FAILURE_ACTIONSW {
        dwResetPeriod: policy.reset_period_secs,
        lpRebootMsg: PWSTR::null(),
        lpCommand: PWSTR::null(),
        cActions: actions.len() as u32,
        lpsaActions: actions.as_mut_ptr(),
    };
    // SAFETY: the action slice outlives the call and the handle is open.
    let configured = unsafe {
        ChangeServiceConfig2W(
            service,
            SERVICE_CONFIG_FAILURE_ACTIONS,
            Some(std::ptr::addr_of!(failure).cast()),
        )
    };
    if let Err(error) = configured {
        tracing::warn!(%error, "could not register service restart policy");
    }
}

pub fn uninstall() -> io::Result<()> {
    let name = wide(OsStr::new(SERVICE_NAME));
    // SAFETY: all SCM handles and the output status remain valid for the calls.
    unsafe {
        let manager = ServiceHandle(
            OpenSCManagerW(PCWSTR::null(), PCWSTR::null(), SC_MANAGER_CONNECT).map_err(io_error)?,
        );
        let service = ServiceHandle(
            OpenServiceW(manager.0, PCWSTR(name.as_ptr()), SERVICE_STOP | 0x0001_0000)
                .map_err(io_error)?,
        );
        let mut status = SERVICE_STATUS::default();
        if let Err(error) = ControlService(service.0, SERVICE_CONTROL_STOP, &mut status) {
            if error.code() != ERROR_SERVICE_NOT_ACTIVE.to_hresult() {
                return Err(io_error(error));
            }
        }
        DeleteService(service.0).map_err(io_error)
    }
}

static STOP_REQUESTED: AtomicBool = AtomicBool::new(false);

extern "system" fn control_handler(control: u32) {
    if control == SERVICE_CONTROL_STOP {
        STOP_REQUESTED.store(true, Ordering::Release);
    }
}

fn report_status(
    handle: SERVICE_STATUS_HANDLE,
    state: SERVICE_STATUS_CURRENT_STATE,
    exit_code: u32,
) -> io::Result<()> {
    let status = SERVICE_STATUS {
        dwServiceType: SERVICE_WIN32_OWN_PROCESS,
        dwCurrentState: state,
        dwControlsAccepted: if state == SERVICE_RUNNING {
            SERVICE_ACCEPT_STOP | SERVICE_ACCEPT_SESSIONCHANGE
        } else {
            0
        },
        dwWin32ExitCode: exit_code,
        ..Default::default()
    };
    // SAFETY: SCM supplied the status handle; status is initialized and alive during the call.
    unsafe { SetServiceStatus(handle, &status).map_err(io_error) }
}

fn stop_worker(worker: &mut Option<WorkerHandle>) -> io::Result<()> {
    if let Some(handle) = worker.as_ref() {
        if let Err(error) = stop_session_worker(handle) {
            if worker_is_alive(handle) {
                return Err(error);
            }
        }
        *worker = None;
    }
    Ok(())
}

fn supervise(worker: &mut Option<WorkerHandle>) -> io::Result<()> {
    let state = worker.as_ref().map(|handle| WorkerState {
        session_id: handle.session_id,
        desktop: handle.desktop,
    });
    let console = console_state()?;
    let action = crate::windows_session::decide_action(console, state);
    service_log(&format!(
        "tick console={console:?} worker={state:?} action={action:?}"
    ));
    match action {
        SupervisorAction::Idle => {
            if worker
                .as_ref()
                .is_some_and(|handle| !worker_is_alive(handle))
            {
                *worker = None;
            }
        }
        SupervisorAction::Spawn {
            session_id,
            desktop,
        } => {
            let spawned = spawn_session_worker(session_id, desktop);
            match &spawned {
                Ok(handle) => service_log(&format!("spawned worker pid={}", handle.process_id)),
                Err(error) => service_log(&format!("spawn failed: {error}")),
            }
            *worker = Some(spawned?);
        }
        SupervisorAction::Respawn {
            session_id,
            desktop,
        } => {
            stop_worker(worker)?;
            *worker = Some(spawn_session_worker(session_id, desktop)?);
        }
        SupervisorAction::StopWorker => stop_worker(worker)?,
    }
    Ok(())
}

extern "system" fn service_main(_argc: u32, _argv: *mut PWSTR) {
    STOP_REQUESTED.store(false, Ordering::Release);
    let name = wide(OsStr::new(SERVICE_NAME));
    // SAFETY: the callback has the SCM ABI and the service name is NUL-terminated.
    let status_handle = match unsafe {
        RegisterServiceCtrlHandlerW(PCWSTR(name.as_ptr()), Some(control_handler))
    } {
        Ok(handle) => handle,
        Err(error) => {
            tracing::error!(%error, "registering service handler failed");
            return;
        }
    };
    if let Err(error) = report_status(status_handle, SERVICE_RUNNING, 0) {
        tracing::error!(%error, "reporting running service failed");
        if let Err(error) = report_status(
            status_handle,
            SERVICE_STOPPED,
            ERROR_SERVICE_SPECIFIC_ERROR.0,
        ) {
            tracing::error!(%error, "reporting stopped service failed");
        }
        return;
    }
    let mut worker = None;
    while !STOP_REQUESTED.load(Ordering::Acquire) {
        if let Err(error) = supervise(&mut worker) {
            service_log(&format!("supervision failed: {error}"));
            tracing::error!(%error, "session worker supervision failed");
        }
        std::thread::sleep(Duration::from_millis(1000));
    }
    let exit_code = match stop_worker(&mut worker) {
        Ok(()) => 0,
        Err(error) => {
            tracing::error!(%error, "stopping session worker failed");
            error
                .raw_os_error()
                .map_or(ERROR_GEN_FAILURE.0, |code| code as u32)
        }
    };
    if let Err(error) = report_status(status_handle, SERVICE_STOPPED, exit_code) {
        tracing::error!(%error, "reporting stopped service failed");
    }
}

pub fn run_service_dispatcher() -> io::Result<()> {
    let mut name = wide(OsStr::new(SERVICE_NAME));
    let table = [
        SERVICE_TABLE_ENTRYW {
            lpServiceName: PWSTR(name.as_mut_ptr()),
            lpServiceProc: Some(service_main),
        },
        SERVICE_TABLE_ENTRYW::default(),
    ];
    // SAFETY: the table is terminated, the callback has the SCM ABI, and buffers outlive dispatch.
    unsafe { StartServiceCtrlDispatcherW(table.as_ptr()).map_err(io_error) }
}

/// `OpenInputDesktop` is session-local: from the service's session 0 it always
/// fails, and `WTSQuerySessionInformationW` lock flags were measured not to move
/// on a console session with autologon, so neither can discriminate the desktop
/// from here. The session worker runs inside the console session and writes the
/// desktop name it observes; the supervisor reads that hint.
pub(crate) fn desktop_hint_path() -> std::path::PathBuf {
    std::env::var("ProgramData")
        .map(|root| Path::new(&root).join("MahoRD").join("input-desktop.txt"))
        .unwrap_or_else(|_| Path::new("C:\\ProgramData\\MahoRD\\input-desktop.txt").to_path_buf())
}

/// Reports the input desktop of the calling process's session. Called by the
/// session worker, which is the only component that can see it.
/// Reports the input desktop of the calling process's session. Called by the
/// session worker, which is the only component that can see it.
///
/// Spawns a background thread that keeps republishing, because the desktop the
/// worker was launched on can change under it: when the console switches to the
/// secure desktop the worker's `OpenInputDesktop` starts failing, and that
/// failure is itself the signal the supervisor needs to respawn onto Winlogon.
pub fn publish_input_desktop() {
    write_input_desktop();
    let _ = std::thread::Builder::new()
        .name("maho-desktop-hint".into())
        .spawn(|| loop {
            std::thread::sleep(Duration::from_millis(500));
            write_input_desktop();
        });
}

fn write_input_desktop() {
    let path = desktop_hint_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    // An unreadable input desktop means the console switched to the secure
    // desktop, which this worker has no access to; report it as Winlogon so the
    // supervisor respawns a worker that does.
    let name = current_input_desktop_name().unwrap_or_else(|| "Winlogon".to_owned());
    let _ = std::fs::write(&path, name);
}

fn current_input_desktop_name() -> Option<String> {
    // SAFETY: the desktop handle is closed below; no inherited access is requested.
    let desktop = unsafe { OpenInputDesktop(DESKTOP_CONTROL_FLAGS(0), false, DESKTOP_READOBJECTS) }
        .ok()
        .map(DesktopHandle)?;
    let mut needed = 0;
    // SAFETY: a zero-sized query writes only the required size to the valid output pointer.
    let _ = unsafe {
        GetUserObjectInformationW(HANDLE(desktop.0 .0), UOI_NAME, None, 0, Some(&mut needed))
    };
    let mut name = vec![0u16; (needed as usize).div_ceil(2).max(1)];
    // SAFETY: name holds at least needed bytes and the desktop stays open for the query.
    unsafe {
        GetUserObjectInformationW(
            HANDLE(desktop.0 .0),
            UOI_NAME,
            Some(name.as_mut_ptr().cast()),
            needed,
            Some(&mut needed),
        )
    }
    .ok()?;
    let end = name
        .iter()
        .position(|unit| *unit == 0)
        .unwrap_or(name.len());
    Some(String::from_utf16_lossy(&name[..end]))
}

fn session_desktop_kind(_session_id: u32) -> Option<DesktopKind> {
    let raw = std::fs::read_to_string(desktop_hint_path()).ok()?;
    let name = raw.trim();
    if name.is_empty() {
        return None;
    }
    Some(crate::windows_session::desktop_kind(name))
}

pub fn console_state() -> io::Result<ConsoleState> {
    // SAFETY: this API has no pointer arguments or prerequisites.
    let session = unsafe { WTSGetActiveConsoleSessionId() };
    let session_id = (session != u32::MAX).then_some(session);
    if let Some(session) = session_id {
        if let Some(desktop) = session_desktop_kind(session) {
            return Ok(ConsoleState {
                session_id,
                desktop,
            });
        }
    }
    // SAFETY: the returned desktop is owned below; no inherited access is requested.
    let desktop =
        match unsafe { OpenInputDesktop(DESKTOP_CONTROL_FLAGS(0), false, DESKTOP_READOBJECTS) } {
            Ok(handle) => DesktopHandle(handle),
            Err(_) => {
                return Ok(ConsoleState {
                    session_id,
                    desktop: DesktopKind::Winlogon,
                });
            }
        };
    let mut needed = 0;
    // SAFETY: a zero-sized query writes only the required size to the valid output pointer.
    let sizing = unsafe {
        GetUserObjectInformationW(HANDLE(desktop.0 .0), UOI_NAME, None, 0, Some(&mut needed))
    };
    if let Err(error) = sizing {
        if error.code() != ERROR_INSUFFICIENT_BUFFER.to_hresult() {
            return Err(io_error(error));
        }
    }
    let mut name = vec![0u16; (needed as usize).div_ceil(2)];
    // SAFETY: name has at least needed bytes and the desktop remains open during the query.
    unsafe {
        GetUserObjectInformationW(
            HANDLE(desktop.0 .0),
            UOI_NAME,
            Some(name.as_mut_ptr().cast()),
            needed,
            Some(&mut needed),
        )
        .map_err(io_error)?;
    }
    let end = name.iter().position(|c| *c == 0).unwrap_or(name.len());
    let desktop = crate::windows_session::desktop_kind(&String::from_utf16_lossy(&name[..end]));
    Ok(ConsoleState {
        session_id,
        desktop,
    })
}

/// Duplicates the service's SYSTEM identity, not the logged-on user's token.
pub fn spawn_session_worker(session_id: u32, desktop: DesktopKind) -> io::Result<WorkerHandle> {
    let exe = std::env::current_exe()?;
    let application = wide(exe.as_os_str());
    let mut command = command_line(&exe, "--session-worker")?;
    let mut desktop_name = wide(OsStr::new(desktop.startup_desktop()));
    // SAFETY: all output pointers refer to initialized storage; owned tokens and buffers outlive process creation.
    unsafe {
        let mut token = HANDLE::default();
        OpenProcessToken(
            GetCurrentProcess(),
            TOKEN_DUPLICATE | TOKEN_QUERY | TOKEN_ASSIGN_PRIMARY,
            &mut token,
        )
        .map_err(io_error)?;
        let token = KernelHandle(token);
        let mut primary = HANDLE::default();
        DuplicateTokenEx(
            token.0,
            TOKEN_ALL_ACCESS,
            None,
            SecurityImpersonation,
            TokenPrimary,
            &mut primary,
        )
        .map_err(io_error)?;
        let primary = KernelHandle(primary);
        SetTokenInformation(
            primary.0,
            TokenSessionId,
            (&session_id as *const u32).cast(),
            size_of::<u32>() as u32,
        )
        .map_err(io_error)?;
        let mut environment = std::ptr::null_mut();
        let environment = match CreateEnvironmentBlock(&mut environment, Some(primary.0), false) {
            Ok(()) => Some(Environment(environment)),
            Err(error) => {
                tracing::warn!(%error, "worker environment unavailable; inheriting service environment");
                None
            }
        };
        let startup = STARTUPINFOW {
            cb: size_of::<STARTUPINFOW>() as u32,
            lpDesktop: PWSTR(desktop_name.as_mut_ptr()),
            ..Default::default()
        };
        let mut process = PROCESS_INFORMATION::default();
        CreateProcessAsUserW(
            Some(primary.0),
            PCWSTR(application.as_ptr()),
            Some(PWSTR(command.as_mut_ptr())),
            None,
            None,
            false,
            CREATE_UNICODE_ENVIRONMENT | CREATE_NO_WINDOW | CREATE_NEW_CONSOLE,
            environment.as_ref().map(|block| block.0.cast_const()),
            PCWSTR::null(),
            &startup,
            &mut process,
        )
        .map_err(io_error)?;
        let _process = KernelHandle(process.hProcess);
        let _thread = KernelHandle(process.hThread);
        Ok(WorkerHandle {
            process_id: process.dwProcessId,
            session_id,
            desktop,
        })
    }
}

pub fn stop_session_worker(handle: &WorkerHandle) -> io::Result<()> {
    // SAFETY: the process handle is opened with termination access and owned until the call completes.
    unsafe {
        let process = KernelHandle(
            OpenProcess(PROCESS_TERMINATE, false, handle.process_id).map_err(io_error)?,
        );
        TerminateProcess(process.0, 0).map_err(io_error)
    }
}

pub fn worker_is_alive(handle: &WorkerHandle) -> bool {
    // SAFETY: the process handle has query access and the exit-code output is valid.
    unsafe {
        let process = match OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, handle.process_id)
        {
            Ok(process) => KernelHandle(process),
            Err(error) => {
                tracing::debug!(%error, process_id = handle.process_id, "worker process unavailable");
                return false;
            }
        };
        let mut code = 0;
        match GetExitCodeProcess(process.0, &mut code) {
            Ok(()) => code == STILL_ACTIVE.0 as u32,
            Err(error) => {
                tracing::warn!(%error, "querying worker exit code failed");
                false
            }
        }
    }
}
