use serde::Serialize;

#[cfg(windows)]
#[path = "native_keys_windows.rs"]
mod platform;

#[derive(Debug, Serialize)]
#[serde(tag = "status", content = "reason")]
pub enum CaptureError {
    Unsupported(String),
    PermissionDenied(String),
    Failed(String),
}

impl std::fmt::Display for CaptureError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unsupported(reason) | Self::PermissionDenied(reason) | Self::Failed(reason) => {
                f.write_str(reason)
            }
        }
    }
}

pub fn setup(app: &tauri::App) {
    #[cfg(windows)]
    platform::setup(app);
    #[cfg(not(windows))]
    let _ = app;
}

#[tauri::command]
pub fn start_key_capture(window: tauri::WebviewWindow) -> Result<(), CaptureError> {
    if window.label() != "main" {
        return Err(CaptureError::PermissionDenied(
            "Only the main remote viewport may capture keys".into(),
        ));
    }
    if !window
        .is_focused()
        .map_err(|e| CaptureError::Failed(e.to_string()))?
    {
        return Err(CaptureError::PermissionDenied(
            "The main window must be focused".into(),
        ));
    }
    #[cfg(windows)]
    {
        platform::start(window)
    }
    #[cfg(target_os = "macos")]
    {
        Err(CaptureError::Unsupported("macOS system-key interception requires Accessibility/Input Monitoring authorization. TCC permission requests are forbidden; no event tap is installed.".into()))
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    Err(CaptureError::Unsupported(
        "Native system-key capture is not implemented for this platform".into(),
    ))
}

#[tauri::command]
pub fn stop_key_capture(app: tauri::AppHandle) -> Result<(), CaptureError> {
    #[cfg(windows)]
    {
        platform::stop(&app)
    }
    #[cfg(not(windows))]
    {
        let _ = app;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unsupported_is_machine_readable() {
        let error = CaptureError::Unsupported("unavailable".into());
        let json = serde_json::to_value(error).unwrap();
        assert_eq!(json["status"], "Unsupported");
        assert_eq!(json["reason"], "unavailable");
    }
}
