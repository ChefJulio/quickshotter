//! Unified coordinate conversion for window positioning.
//!
//! Three coordinate spaces exist in QuickShotter:
//!
//! 1. **Physical screen pixels** — the virtual desktop coordinate space used
//!    by xcap, Win32 `GetWindowRect`/`GetCursorPos`, and our capture pipeline.
//!    This is the ground truth.
//!
//! 2. **Overlay CSS pixels** — the webview's coordinate system. Related to
//!    physical by: `physical = css * dpr + desktop_origin`, where `dpr` is
//!    `window.devicePixelRatio` (== `xcapScale` on Windows).
//!
//! 3. **Tauri logical** — what `.position()` and `.inner_size()` expect.
//!    Tauri converts logical to physical via `physical = logical * scale_factor`
//!    using the target monitor's DPI scale factor.
//!
//! All coordinate math MUST go through the functions in this module.

use tauri::AppHandle;

/// A point in physical (virtual desktop) screen coordinates.
#[derive(Debug, Clone, Copy)]
pub struct ScreenPoint {
  pub x: f64,
  pub y: f64,
}

/// A size in physical screen pixels.
#[derive(Debug, Clone, Copy)]
pub struct ScreenSize {
  pub w: f64,
  pub h: f64,
}

/// Convert overlay CSS coordinates to physical screen coordinates.
///
/// The overlay webview spans the full virtual desktop, positioned at the
/// desktop bounds origin.  CSS coordinates relate to physical as:
///   `physical = css * dpr + origin`
///
/// - `dpr`: `window.devicePixelRatio` from the overlay (== xcapScale on Windows, 1.0 on macOS)
/// - `origin`: desktop bounds origin from `get_desktop_bounds()`
#[allow(dead_code)]
pub fn css_to_screen(css_x: f64, css_y: f64, dpr: f64, origin: ScreenPoint) -> ScreenPoint {
  ScreenPoint {
    x: css_x * dpr + origin.x,
    y: css_y * dpr + origin.y,
  }
}

/// Convert a physical screen point to Tauri logical coordinates for `.position()`.
///
/// Tauri interprets position values as logical and converts via:
///   `physical = logical * scale_factor`
/// using the target monitor's DPI.  To place a window at physical point P:
///   `logical = P / scale_factor`
pub fn to_tauri_pos(point: ScreenPoint, app: &AppHandle) -> (f64, f64) {
  let sf = scale_factor_at(point, app);
  (point.x / sf, point.y / sf)
}

/// Convert a physical size to Tauri logical for `.inner_size()`.
/// Uses the scale factor of the monitor at `at`.
pub fn to_tauri_size(size: ScreenSize, at: ScreenPoint, app: &AppHandle) -> (f64, f64) {
  let sf = scale_factor_at(at, app);
  (size.w / sf, size.h / sf)
}

/// Clamp a window so it stays within the monitor containing `anchor`.
///
/// Returns the clamped position and (potentially trimmed) size.
/// `anchor` determines which monitor to clamp to — typically the center
/// or origin of the selection being bordered.
pub fn clamp_to_monitor(
  pos: ScreenPoint,
  size: ScreenSize,
  anchor: ScreenPoint,
  app: &AppHandle,
) -> (ScreenPoint, ScreenSize) {
  if let Some((mx, my, mw, mh)) = monitor_phys_rect(anchor, app) {
    // Trim size if larger than monitor
    let w = size.w.min(mw);
    let h = size.h.min(mh);
    let x = pos.x.max(mx).min(mx + mw - w);
    let y = pos.y.max(my).min(my + mh - h);
    (ScreenPoint { x, y }, ScreenSize { w, h })
  } else {
    (pos, size)
  }
}

/// Get the scale factor of the monitor containing a physical screen point.
fn scale_factor_at(point: ScreenPoint, app: &AppHandle) -> f64 {
  if let Ok(monitors) = app.available_monitors() {
    for mon in &monitors {
      let pos = mon.position();
      let size = mon.size();
      if point.x >= pos.x as f64
        && point.x < pos.x as f64 + size.width as f64
        && point.y >= pos.y as f64
        && point.y < pos.y as f64 + size.height as f64
      {
        return mon.scale_factor();
      }
    }
  }
  1.0
}

/// Get the physical bounding rect (x, y, w, h) of the monitor containing a point.
fn monitor_phys_rect(point: ScreenPoint, app: &AppHandle) -> Option<(f64, f64, f64, f64)> {
  let monitors = app.available_monitors().ok()?;
  for mon in &monitors {
    let pos = mon.position();
    let size = mon.size();
    let mx = pos.x as f64;
    let my = pos.y as f64;
    let mw = size.width as f64;
    let mh = size.height as f64;
    if point.x >= mx && point.x < mx + mw && point.y >= my && point.y < my + mh {
      return Some((mx, my, mw, mh));
    }
  }
  None
}
