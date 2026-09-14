#[cfg(target_os = "linux")]
pub mod audio_linux;
#[cfg(target_os = "windows")]
pub mod audio_windows;
#[cfg(target_os = "linux")]
pub mod capture_linux;
#[cfg(target_os = "macos")]
pub mod capture_macos;
#[cfg(target_os = "windows")]
pub mod capture_windows;
#[cfg(target_os = "linux")]
pub mod clipboard_linux;
#[cfg(target_os = "windows")]
pub mod clipboard_windows;
#[cfg(target_os = "linux")]
pub mod encode_linux;
#[cfg(target_os = "macos")]
pub mod encode_vt;
#[cfg(target_os = "windows")]
pub mod encode_windows;
#[cfg(target_os = "linux")]
pub mod inject_linux;
#[cfg(target_os = "macos")]
pub mod inject_macos;
#[cfg(target_os = "windows")]
pub mod inject_windows;
#[cfg(target_os = "windows")]
pub mod service_windows;
pub mod session;
pub mod windows_logic;
pub mod windows_session;

#[cfg(test)]
#[path = "../../test-support/allocations.rs"]
mod test_alloc;

#[cfg(target_os = "macos")]
pub use capture_macos::{CaptureConfig, CaptureEvent, CaptureFrame, ScreenCapture};
#[cfg(target_os = "windows")]
pub use capture_windows::WindowsCapture;
#[cfg(target_os = "windows")]
pub use clipboard_windows::WindowsClipboard;
#[cfg(target_os = "macos")]
pub use encode_vt::{EncodedFrame, EncoderConfig, VideoToolboxEncoder};
#[cfg(target_os = "windows")]
pub use encode_windows::MediaFoundationEncoder;
#[cfg(target_os = "macos")]
pub use inject_macos::{accessibility_is_trusted, request_accessibility, InputInjector};
#[cfg(target_os = "windows")]
pub use inject_windows::WindowsInputInjector;

/// Builds the Windows host configuration for a service-spawned session worker,
/// waiting for DXGI to describe an output instead of failing.
///
/// A worker spawned onto the secure desktop starts before any output can be
/// enumerated. Returning an error there would exit the process and the service
/// would respawn it immediately, so this retries until the desktop becomes
/// capturable or the startup budget runs out.
#[cfg(target_os = "windows")]
pub fn windows_default_blocking(
    bootstrap_pin: Option<String>,
    pairing_store: session::PairingStore,
) -> Result<session::HostConfig, session::SessionError> {
    let started = std::time::Instant::now();
    loop {
        match session::HostConfig::windows_default(bootstrap_pin.clone(), pairing_store.clone()) {
            Ok(config) => return Ok(config),
            Err(error) => {
                if !windows_session::keep_waiting_for_output(started.elapsed()) {
                    return Err(error);
                }
                tracing::debug!(%error, "waiting for a capturable output");
                std::thread::sleep(windows_session::WORKER_OUTPUT_POLL);
            }
        }
    }
}
#[cfg(target_os = "linux")]
pub use session::{focused_output_name, probe_hyprland_monitors, resolve_output_target};
pub use session::{
    random_pin, select_focused_output, ConsentPrompt, DisplayInfo, HostConfig, HostServer,
    PairingRecord, PairingStore, SessionState, TimestampStats, VideoFrame,
};
