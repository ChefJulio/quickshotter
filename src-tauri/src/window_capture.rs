/// Cross-platform window detection for window capture mode.
///
/// Uses xcap::Window::all() (sorted by z-order, topmost first) to find the
/// topmost visible, non-minimized window at a given screen coordinate.
/// Uses mouse_position crate for cross-platform cursor position.

use xcap::Window;

#[derive(Debug, Clone, serde::Serialize)]
pub struct WindowRect {
  pub left: i32,
  pub top: i32,
  pub right: i32,
  pub bottom: i32,
}

/// Cross-platform window identifier (u32 from xcap).
pub type WindowId = u32;

pub fn get_cursor_pos() -> (i32, i32) {
  use mouse_position::mouse_position::Mouse;
  match Mouse::get_mouse_position() {
    Mouse::Position { x, y } => (x, y),
    Mouse::Error => (0, 0),
  }
}

pub fn get_window_rect_at(x: i32, y: i32, exclude_id: WindowId) -> Option<WindowRect> {
  let windows = match Window::all() {
    Ok(w) => w,
    Err(e) => {
      eprintln!("window_capture: Window::all() failed: {e}");
      return None;
    }
  };

  // Window::all() returns windows sorted by z-order (topmost first).
  // Find first (topmost) visible window containing the point.
  for win in &windows {
    let win_id = match win.id() {
      Ok(id) => id,
      Err(_) => continue,
    };
    if exclude_id != 0 && win_id == exclude_id {
      continue;
    }

    if win.is_minimized().unwrap_or(false) {
      continue;
    }

    // Skip windows with empty titles (typically desktop/shell windows)
    let title = win.title().unwrap_or_default();
    if title.is_empty() {
      continue;
    }

    let wx = match win.x() { Ok(v) => v, Err(_) => continue };
    let wy = match win.y() { Ok(v) => v, Err(_) => continue };
    let ww = match win.width() { Ok(v) => v as i32, Err(_) => continue };
    let wh = match win.height() { Ok(v) => v as i32, Err(_) => continue };

    if ww < 1 || wh < 1 {
      continue;
    }

    // Hit test: is (x, y) within this window's bounds?
    if wx <= x && x < wx + ww && wy <= y && y < wy + wh {
      return Some(WindowRect {
        left: wx,
        top: wy,
        right: wx + ww,
        bottom: wy + wh,
      });
    }
  }

  None
}
