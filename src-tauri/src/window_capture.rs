/// Cross-platform window detection for window capture mode.
///
/// Uses xcap::Window::all() (sorted by z-order, topmost first) to find the
/// topmost visible, non-minimized window at a given screen coordinate.
/// Uses mouse_position crate for cross-platform cursor position.
///
/// A persistent worker thread handles queries to avoid ~1ms thread-spawn
/// overhead on every cursor poll during window capture mode.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, OnceLock};
use xcap::Window;

static WINDOW_QUERY_ACTIVE: AtomicBool = AtomicBool::new(false);

struct WindowQuery {
  x: i32,
  y: i32,
  exclude_id: WindowId,
  reply: mpsc::Sender<Option<WindowRect>>,
}

static WORKER: OnceLock<mpsc::Sender<WindowQuery>> = OnceLock::new();

fn get_or_init_worker() -> &'static mpsc::Sender<WindowQuery> {
  WORKER.get_or_init(|| {
    let (tx, rx) = mpsc::channel::<WindowQuery>();
    std::thread::Builder::new()
      .name("window-detect".into())
      .spawn(move || {
        for query in rx {
          let result = find_window_at(query.x, query.y, query.exclude_id);
          WINDOW_QUERY_ACTIVE.store(false, Ordering::SeqCst);
          query.reply.send(result).ok();
        }
      })
      .expect("failed to spawn window detection thread");
    tx
  })
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

pub fn get_cursor_pos() -> (i32, i32) {
  use mouse_position::mouse_position::Mouse;
  match Mouse::get_mouse_position() {
    Mouse::Position { x, y } => (x, y),
    Mouse::Error => (0, 0),
  }
}

pub fn get_window_rect_at(x: i32, y: i32, exclude_id: WindowId) -> Option<WindowRect> {
  // Skip if a previous query is still being processed
  if WINDOW_QUERY_ACTIVE.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst).is_err() {
    return None;
  }

  let worker = get_or_init_worker();
  let (reply_tx, reply_rx) = mpsc::channel();
  if worker.send(WindowQuery { x, y, exclude_id, reply: reply_tx }).is_err() {
    WINDOW_QUERY_ACTIVE.store(false, Ordering::SeqCst);
    return None;
  }

  match reply_rx.recv_timeout(std::time::Duration::from_millis(500)) {
    Ok(result) => result,
    Err(_) => {
      // Worker thread will reset WINDOW_QUERY_ACTIVE when it finishes
      eprintln!("window_capture: Window::all() timed out");
      None
    }
  }
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
