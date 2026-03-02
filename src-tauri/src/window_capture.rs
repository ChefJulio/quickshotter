/// Cross-platform window detection for window capture mode.
///
/// Uses xcap::Window::all() (sorted by z-order, topmost first) to find the
/// topmost visible, non-minimized window at a given screen coordinate.
/// Uses mouse_position crate for cross-platform cursor position.
///
/// Architecture: a background worker thread continuously calls Window::all()
/// (which can take 500ms+ on some systems) and caches the latest result.
/// The Tauri command reads the cache instantly -- no blocking, no timeouts.

use std::sync::{Arc, Mutex as StdMutex, OnceLock};
use std::sync::atomic::{AtomicBool, Ordering};
use xcap::Window;

/// Shared state between the worker thread and the Tauri command.
struct DetectState {
  /// Current cursor position (updated by the command).
  cursor_x: i32,
  cursor_y: i32,
  /// Window ID to exclude from detection (the overlay).
  exclude_id: u32,
  /// Latest detection result (updated by the worker).
  result: Option<WindowRect>,
  /// Whether window capture mode is active.
  active: bool,
}

static SHARED: OnceLock<Arc<StdMutex<DetectState>>> = OnceLock::new();
static WORKER_SPAWNED: AtomicBool = AtomicBool::new(false);

fn shared() -> &'static Arc<StdMutex<DetectState>> {
  SHARED.get_or_init(|| {
    Arc::new(StdMutex::new(DetectState {
      cursor_x: 0,
      cursor_y: 0,
      exclude_id: 0,
      result: None,
      active: false,
    }))
  })
}

fn ensure_worker() {
  if WORKER_SPAWNED.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst).is_ok() {
    let state = shared().clone();
    std::thread::Builder::new()
      .name("window-detect".into())
      .spawn(move || {
        loop {
          let (x, y, exclude, active) = {
            let s = state.lock().unwrap();
            (s.cursor_x, s.cursor_y, s.exclude_id, s.active)
          };

          if active {
            let result = find_window_at(x, y, exclude);
            state.lock().unwrap().result = result;
          } else {
            // Sleep longer when inactive to avoid wasting CPU
            std::thread::sleep(std::time::Duration::from_millis(100));
          }
        }
      })
      .expect("failed to spawn window detection thread");
  }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct WindowRect {
  pub left: i32,
  pub top: i32,
  pub right: i32,
  pub bottom: i32,
}

/// Cross-platform window identifier (u32 from xcap).
pub type WindowId = u32;

pub fn get_cursor_pos() -> Option<(i32, i32)> {
  use mouse_position::mouse_position::Mouse;
  match Mouse::get_mouse_position() {
    Mouse::Position { x, y } => {
      // On macOS, mouse_position returns Cocoa coordinates (y=0 at bottom
      // of primary display), but xcap::Window uses CG coordinates (y=0 at
      // top). Flip y so hit-testing matches the window positions.
      #[cfg(target_os = "macos")]
      {
        let primary_h = xcap::Monitor::all()
          .ok()
          .and_then(|m| m.first().map(|mon| mon.height().unwrap_or(0) as i32))
          .unwrap_or(0);
        if primary_h > 0 {
          return Some((x, primary_h - y));
        }
      }
      Some((x, y))
    }
    Mouse::Error => None,
  }
}

/// Start the detection worker (call when entering window capture mode).
pub fn start() {
  let s = shared();
  s.lock().unwrap().active = true;
  ensure_worker();
}

/// Stop the detection worker (call when leaving window capture mode).
pub fn stop() {
  let s = shared();
  let mut state = s.lock().unwrap();
  state.active = false;
  state.result = None;
}

/// Update cursor position and return the latest cached detection result.
/// Returns instantly -- never blocks on Window::all().
pub fn get_window_rect_at(x: i32, y: i32, exclude_id: WindowId) -> Option<WindowRect> {
  let s = shared();
  let mut state = s.lock().unwrap();
  state.cursor_x = x;
  state.cursor_y = y;
  state.exclude_id = exclude_id;
  state.result.clone()
}

fn find_window_at(x: i32, y: i32, exclude_id: WindowId) -> Option<WindowRect> {
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
