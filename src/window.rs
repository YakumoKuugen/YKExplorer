//! Window lifecycle helpers.
//!
//! A single process can own any number of explorer windows. Every window keeps
//! its own `ExplorerApp` (tabs, history, selection) while sharing the on-disk
//! configuration. Window geometry is persisted separately so the preferences
//! file is never rewritten just because a window moved.

use std::path::PathBuf;

use gpui::{
    App, AppContext as _, Bounds, Entity, Window, WindowBounds, WindowHandle, WindowOptions, point,
    px, size,
};
use serde::{Deserialize, Serialize};

use crate::app::ExplorerApp;

pub const DEFAULT_WIDTH: f32 = 1440.0;
pub const DEFAULT_HEIGHT: f32 = 900.0;
pub const MIN_WIDTH: f32 = 1120.0;
pub const MIN_HEIGHT: f32 = 680.0;

/// Serialized position/size of the last closed window.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct WindowGeometry {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub maximized: bool,
}

impl Default for WindowGeometry {
    fn default() -> Self {
        Self {
            x: 0.0,
            y: 0.0,
            width: DEFAULT_WIDTH,
            height: DEFAULT_HEIGHT,
            maximized: false,
        }
    }
}

impl WindowGeometry {
    pub fn from_bounds(bounds: WindowBounds) -> Self {
        let maximized = matches!(bounds, WindowBounds::Maximized(_));
        let bounds = bounds.get_bounds();
        Self {
            x: f32::from(bounds.origin.x),
            y: f32::from(bounds.origin.y),
            width: f32::from(bounds.size.width),
            height: f32::from(bounds.size.height),
            maximized,
        }
    }

    fn is_sane(self) -> bool {
        self.x.is_finite()
            && self.y.is_finite()
            && self.width.is_finite()
            && self.height.is_finite()
            && self.width >= 320.0
            && self.height >= 240.0
    }

    fn to_bounds(self) -> Bounds<gpui::Pixels> {
        Bounds::new(
            point(px(self.x), px(self.y)),
            size(px(self.width), px(self.height)),
        )
    }
}

/// Everything needed to open one explorer window.
#[derive(Clone, Default)]
pub struct WindowSpec {
    /// Folder to show. `None` uses the normal start directory.
    pub path: Option<PathBuf>,
    /// Only the first window of a process should serve the debug IPC port.
    pub debug_port: Option<u16>,
    /// Explicit geometry (used when duplicating a window).
    pub bounds: Option<WindowBounds>,
    /// Restore the previous maximized state. Only the startup window does this;
    /// windows opened from a running instance always start windowed.
    pub restore_maximized: bool,
}

impl WindowSpec {
    pub fn at(path: impl Into<PathBuf>) -> Self {
        Self {
            path: Some(path.into()),
            ..Default::default()
        }
    }
}

fn session_files() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Some(config) = dirs::config_dir() {
        paths.push(config.join("YKExplorer").join("window.json"));
    }
    if let Some(local) = dirs::data_local_dir() {
        let candidate = local.join("YKExplorer").join("window.json");
        if !paths.contains(&candidate) {
            paths.push(candidate);
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            let candidate = parent.join("ykexplorer-window.json");
            if !paths.contains(&candidate) {
                paths.push(candidate);
            }
        }
    }
    paths
}

fn load_geometry() -> Option<WindowGeometry> {
    for path in session_files() {
        let Ok(text) = std::fs::read_to_string(path) else {
            continue;
        };
        let Ok(geometry) = serde_json::from_str::<WindowGeometry>(&text) else {
            continue;
        };
        if geometry.is_sane() {
            return Some(geometry);
        }
    }
    None
}

fn save_geometry(geometry: WindowGeometry) {
    if !geometry.is_sane() {
        log::warn!("refusing to persist insane window geometry: {geometry:?}");
        return;
    }
    let Ok(json) = serde_json::to_string_pretty(&geometry) else {
        return;
    };
    for path in session_files() {
        if let Some(parent) = path.parent() {
            if std::fs::create_dir_all(parent).is_err() {
                continue;
            }
        }
        // Write to a sibling first so a crash mid-write cannot truncate the file.
        let temporary = path.with_extension("json.tmp");
        if std::fs::write(&temporary, &json).is_ok() && std::fs::rename(&temporary, &path).is_ok() {
            log::info!("window geometry saved to {}", path.display());
            return;
        }
    }
    log::warn!("could not persist window geometry in any known location");
}

/// Geometry for the next window: the last closed window, optionally offset so
/// new windows cascade instead of landing exactly on top of each other.
///
/// This deliberately does not query the active window's root view: doing so
/// while the current `ExplorerApp` is being updated would lease the same entity
/// twice (GPUI panics with "already being updated").
fn next_bounds(cx: &mut App, cascade: bool) -> Bounds<gpui::Pixels> {
    let base = load_geometry()
        .filter(|geometry| geometry.is_sane())
        .map(WindowGeometry::to_bounds)
        .unwrap_or_else(|| Bounds::centered(None, size(px(DEFAULT_WIDTH), px(DEFAULT_HEIGHT)), cx));

    let bounds = if cascade {
        static CASCADE: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let step = CASCADE.fetch_add(1, std::sync::atomic::Ordering::Relaxed) % 6 + 1;
        let offset = 28.0 * step as f32;
        Bounds::new(
            point(
                px(f32::from(base.origin.x) + offset),
                px(f32::from(base.origin.y) + offset),
            ),
            base.size,
        )
    } else {
        base
    };

    fit_to_displays(bounds, cx)
}

/// Keep at least a title-bar-sized part of the window on a real display.
fn fit_to_displays(bounds: Bounds<gpui::Pixels>, cx: &App) -> Bounds<gpui::Pixels> {
    let displays = cx.displays();
    if displays.is_empty() {
        return bounds;
    }
    let mut min_x = f32::MAX;
    let mut min_y = f32::MAX;
    let mut max_x = f32::MIN;
    let mut max_y = f32::MIN;
    for display in displays {
        let area = display.bounds();
        min_x = min_x.min(f32::from(area.origin.x));
        min_y = min_y.min(f32::from(area.origin.y));
        max_x = max_x.max(f32::from(area.origin.x) + f32::from(area.size.width));
        max_y = max_y.max(f32::from(area.origin.y) + f32::from(area.size.height));
    }
    let union_width = (max_x - min_x).max(320.0);
    let union_height = (max_y - min_y).max(240.0);
    let width = f32::from(bounds.size.width).clamp(320.0, union_width);
    let height = f32::from(bounds.size.height).clamp(240.0, union_height);

    let x_lo = min_x - width + 180.0;
    let x_hi = (max_x - 180.0).max(x_lo);
    let y_lo = min_y;
    let y_hi = (max_y - 32.0).max(y_lo);
    let x = f32::from(bounds.origin.x).clamp(x_lo, x_hi);
    let y = f32::from(bounds.origin.y).clamp(y_lo, y_hi);

    Bounds::new(point(px(x), px(y)), size(px(width), px(height)))
}

fn install_drag_host(window: &mut Window) {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    if let Ok(handle) = <gpui::Window as HasWindowHandle>::window_handle(window) {
        if let RawWindowHandle::Win32(raw) = handle.as_raw() {
            crate::drag_host::install(windows::Win32::Foundation::HWND(raw.hwnd.get() as *mut _));
        }
    }
}

/// Open an explorer window in the current process.
///
/// `ExplorerApp` is the window's root view, so an app can request more windows
/// simply by calling this again from an action or menu handler.
pub fn open(cx: &mut App, spec: WindowSpec) -> Option<WindowHandle<ExplorerApp>> {
    let bounds = spec
        .bounds
        .unwrap_or_else(|| WindowBounds::Windowed(next_bounds(cx, !spec.restore_maximized)));
    let maximized = spec.restore_maximized
        && load_geometry()
            .map(|geometry| geometry.maximized)
            .unwrap_or(false)
        && spec.bounds.is_none();
    let window_bounds = if maximized {
        WindowBounds::Maximized(bounds.get_bounds())
    } else {
        bounds
    };

    let options = WindowOptions {
        window_bounds: Some(window_bounds),
        titlebar: None,
        window_decorations: Some(gpui::WindowDecorations::Client),
        window_min_size: Some(size(px(MIN_WIDTH), px(MIN_HEIGHT))),
        focus: true,
        show: true,
        is_resizable: true,
        is_movable: true,
        ..Default::default()
    };

    let start_path = spec.path.clone();
    let debug_port = spec.debug_port;
    let result = cx.open_window(options, |window, cx| {
        window.set_window_title("YKExplorer");
        install_drag_host(window);

        let app: Entity<ExplorerApp> = cx.new(|cx| ExplorerApp::new_at(cx, start_path.clone()));

        #[cfg(debug_assertions)]
        if let Some(port) = debug_port {
            let requests = crate::debug_ipc::start(port);
            app.update(cx, |app: &mut ExplorerApp, cx| {
                app.start_debug_worker(requests, cx)
            });
        }
        #[cfg(not(debug_assertions))]
        let _ = debug_port;

        // Persisting on close keeps the last window's geometry without
        // touching `config.json` when the user only moves a window.
        window.on_window_should_close(cx, |window, cx| {
            let remaining = cx.windows().len();
            if remaining <= 1 {
                let geometry = WindowGeometry::from_bounds(window.window_bounds());
                log::info!("saving last window geometry: {geometry:?}");
                save_geometry(geometry);
            }
            true
        });

        app
    });

    match result {
        Ok(handle) => Some(handle),
        Err(error) => {
            log::error!("open window failed: {error:#}");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn geometry_round_trips_through_json() {
        let geometry = WindowGeometry {
            x: -120.0,
            y: 48.0,
            width: 1280.0,
            height: 760.0,
            maximized: true,
        };
        let json = serde_json::to_string(&geometry).unwrap();
        let decoded: WindowGeometry = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.x, -120.0);
        assert_eq!(decoded.width, 1280.0);
        assert!(decoded.maximized);
        assert!(decoded.is_sane());
    }

    #[test]
    fn invalid_geometry_is_rejected() {
        let mut geometry = WindowGeometry::default();
        geometry.width = f32::NAN;
        assert!(!geometry.is_sane());
        geometry.width = 120.0;
        assert!(!geometry.is_sane());
    }
}
