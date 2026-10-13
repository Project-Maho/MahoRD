use tauri::{
    menu::{CheckMenuItem, Menu, MenuItem},
    tray::TrayIconBuilder,
    Emitter, Manager, State, WindowEvent,
};
use tauri_plugin_autostart::ManagerExt;

struct TrayState {
    hosting: CheckMenuItem<tauri::Wry>,
}

#[tauri::command]
pub fn get_autostart(app: tauri::AppHandle) -> Result<bool, String> {
    app.autolaunch().is_enabled().map_err(|e| e.to_string())
}

#[tauri::command]
pub fn set_autostart(app: tauri::AppHandle, enabled: bool) -> Result<bool, String> {
    let manager = app.autolaunch();
    if enabled {
        manager.enable().map_err(|e| e.to_string())?;
    } else {
        manager.disable().map_err(|e| e.to_string())?;
    }
    manager.is_enabled().map_err(|e| e.to_string())
}

#[tauri::command]
pub fn sync_tray_state(
    app: tauri::AppHandle,
    state: State<'_, crate::AppState>,
) -> Result<(), String> {
    let running = state.get_host_status()?.running;
    let tray = app.state::<TrayState>();
    tray.hosting
        .set_checked(running)
        .map_err(|e| e.to_string())?;
    tray.hosting
        .set_text(if running {
            "Hosting: On"
        } else {
            "Hosting: Off"
        })
        .map_err(|e| e.to_string())
}

pub fn setup(app: &tauri::App) -> Result<(), Box<dyn std::error::Error>> {
    let running = app.state::<crate::AppState>().get_host_status()?.running;
    let open = MenuItem::with_id(app, "native-open", "Open MahoRD", true, None::<&str>)?;
    let hosting = CheckMenuItem::with_id(
        app,
        "native-hosting",
        if running {
            "Hosting: On"
        } else {
            "Hosting: Off"
        },
        true,
        running,
        None::<&str>,
    )?;
    let quit = MenuItem::with_id(app, "native-quit", "Quit", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&open, &hosting, &quit])?;
    app.manage(TrayState { hosting });
    let icon = app
        .default_window_icon()
        .ok_or("missing tray icon")?
        .clone();
    TrayIconBuilder::with_id("mahord-hosting")
        .icon(icon)
        .tooltip("MahoRD")
        .menu(&menu)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "native-open" => {
                if let Some(window) = app.get_webview_window("main") {
                    if let Err(error) = window
                        .show()
                        .and_then(|()| window.unminimize())
                        .and_then(|()| window.set_focus())
                    {
                        tracing::warn!(%error, "Cannot open MahoRD window");
                    }
                }
            }
            "native-hosting" => {
                let handle = app.clone();
                tauri::async_runtime::spawn_blocking(move || {
                    let state = handle.state::<crate::AppState>();
                    let result = state.get_host_status().and_then(|status| {
                        if status.running {
                            state.stop_host()
                        } else {
                            state.start_host()
                        }
                    });
                    match result {
                        Ok(status) => {
                            if let Err(error) = handle.emit("native-host-status", &status) {
                                tracing::warn!(%error, "Cannot publish hosting status");
                            }
                        }
                        Err(error) => {
                            tracing::warn!(%error, "Tray hosting action failed");
                            if let Err(error) = handle.emit("native-host-error", error) {
                                tracing::warn!(%error, "Cannot publish hosting error");
                            }
                        }
                    }
                    if let Err(error) = sync_tray_state(handle.clone(), state) {
                        tracing::warn!(%error, "Cannot synchronize hosting tray");
                    }
                });
            }
            "native-quit" => app.exit(0),
            _ => {}
        })
        .build(app)?;
    let window = app
        .get_webview_window("main")
        .ok_or("missing main window")?;
    let observed = window.clone();
    window.on_window_event(move |event| {
        if let WindowEvent::CloseRequested { api, .. } = event {
            match observed.state::<crate::AppState>().get_host_status() {
                Ok(status) if status.running => {
                    api.prevent_close();
                    if let Err(error) = observed.hide() {
                        tracing::warn!(%error, "Cannot hide hosting window");
                    }
                }
                Ok(_) => {}
                Err(error) => tracing::warn!(%error, "Cannot read hosting state on close"),
            }
        }
    });
    Ok(())
}
