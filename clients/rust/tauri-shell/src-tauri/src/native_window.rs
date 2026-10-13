use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};
use tauri::{Manager, PhysicalPosition, PhysicalSize, WindowEvent};

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
struct Geometry {
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    maximized: bool,
}

fn clamp(mut saved: Geometry, monitors: &[(i32, i32, u32, u32)]) -> Option<Geometry> {
    let &(x, y, width, height) = monitors
        .iter()
        .find(|&&(x, y, w, h)| {
            i64::from(saved.x) >= i64::from(x)
                && i64::from(saved.y) >= i64::from(y)
                && i64::from(saved.x) < i64::from(x) + i64::from(w)
                && i64::from(saved.y) < i64::from(y) + i64::from(h)
        })
        .or_else(|| monitors.first())?;
    saved.width = saved.width.max(320).min(width);
    saved.height = saved.height.max(240).min(height);
    saved.x = i32::try_from(
        i64::from(saved.x).clamp(i64::from(x), i64::from(x) + i64::from(width - saved.width)),
    )
    .ok()?;
    saved.y = i32::try_from(i64::from(saved.y).clamp(
        i64::from(y),
        i64::from(y) + i64::from(height - saved.height),
    ))
    .ok()?;
    Some(saved)
}

pub fn setup(app: &tauri::App) -> Result<(), Box<dyn std::error::Error>> {
    let window = app
        .get_webview_window("main")
        .ok_or("missing main window")?;
    let path = app.path().app_config_dir()?.join("window-geometry.json");
    let monitors: Vec<_> = window
        .available_monitors()?
        .iter()
        .map(|m| {
            (
                m.position().x,
                m.position().y,
                m.size().width,
                m.size().height,
            )
        })
        .collect();
    let mut restored = None;
    if path.exists() {
        match std::fs::read(&path)
            .map_err(|e| e.to_string())
            .and_then(|bytes| serde_json::from_slice::<Geometry>(&bytes).map_err(|e| e.to_string()))
        {
            Ok(saved) => {
                if let Some(saved) = clamp(saved, &monitors) {
                    restored = Some(saved);
                    window.set_size(PhysicalSize::new(saved.width, saved.height))?;
                    window.set_position(PhysicalPosition::new(saved.x, saved.y))?;
                    if saved.maximized {
                        window.maximize()?;
                    }
                }
            }
            Err(error) => tracing::warn!(%error, "Cannot restore window geometry"),
        }
    }
    let size = window.inner_size()?;
    let position = window.outer_position()?;
    let saved = Arc::new(Mutex::new(restored.unwrap_or(Geometry {
        x: position.x,
        y: position.y,
        width: size.width,
        height: size.height,
        maximized: window.is_maximized()?,
    })));
    let observed = window.clone();
    window.on_window_event(move |event| {
        if !matches!(
            event,
            WindowEvent::Moved(_) | WindowEvent::Resized(_) | WindowEvent::CloseRequested { .. }
        ) {
            return;
        }
        let result = (|| -> Result<(), Box<dyn std::error::Error>> {
            if observed.is_fullscreen()? || observed.is_minimized()? {
                return Ok(());
            }
            let mut geometry = saved.lock().map_err(|e| e.to_string())?;
            geometry.maximized = observed.is_maximized()?;
            if !geometry.maximized {
                let position = observed.outer_position()?;
                let size = observed.inner_size()?;
                geometry.x = position.x;
                geometry.y = position.y;
                geometry.width = size.width;
                geometry.height = size.height;
            }
            let parent = path.parent().ok_or("missing geometry directory")?;
            std::fs::create_dir_all(parent)?;
            let temporary = path.with_extension("json.tmp");
            std::fs::write(&temporary, serde_json::to_vec(&*geometry)?)?;
            std::fs::rename(temporary, &path)?;
            Ok(())
        })();
        if let Err(error) = result {
            tracing::warn!(%error, "Cannot save window geometry");
        }
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn removed_monitor_restores_inside_visible_bounds() {
        let saved = Geometry {
            x: 4000,
            y: -2000,
            width: 3000,
            height: 2000,
            maximized: true,
        };
        let restored = clamp(saved, &[(-1920, 0, 1920, 1080)]).unwrap();
        assert_eq!(
            (restored.x, restored.y, restored.width, restored.height),
            (-1920, 0, 1920, 1080)
        );
        assert!(restored.maximized);
    }

    #[test]
    fn connected_secondary_monitor_is_preserved() {
        let saved = Geometry {
            x: -1800,
            y: 100,
            width: 800,
            height: 600,
            maximized: false,
        };
        let restored = clamp(saved, &[(0, 0, 1920, 1080), (-1920, 0, 1920, 1080)]).unwrap();
        assert_eq!((restored.x, restored.y), (-1800, 100));
    }
}
