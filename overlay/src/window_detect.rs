//! Background window detection for window capture mode.
//!
//! Architecture: background thread continuously refreshes the window list
//! via xcap::Window::all(). The main thread does the hit-test against the
//! cached list using the current cursor position. This ensures the hit-test
//! always uses up-to-date cursor coordinates, even though the window list
//! may be slightly stale (50-500ms refresh rate).

use std::sync::{Arc, Mutex};
use xcap::Window;

#[derive(Debug, Clone)]
pub struct WindowRect {
  pub left: i32,
  pub top: i32,
  pub right: i32,
  pub bottom: i32,
}

struct WindowInfo {
  title: String,
  x: i32,
  y: i32,
  w: i32,
  h: i32,
}

struct DetectState {
  /// Cached window list (refreshed by background thread).
  windows: Vec<WindowInfo>,
  active: bool,
}

pub struct WindowDetector {
  state: Arc<Mutex<DetectState>>,
  exclude_prefix: String,
}

impl WindowDetector {
  pub fn new(exclude_prefix: &str) -> Self {
    let state = Arc::new(Mutex::new(DetectState {
      windows: Vec::new(),
      active: true,
    }));

    let thread_state = state.clone();
    std::thread::Builder::new()
      .name("window-detect".into())
      .spawn(move || {
        loop {
          {
            let s = thread_state.lock().unwrap();
            if !s.active { break; }
          }

          // Refresh the window list
          if let Ok(all_windows) = Window::all() {
            let infos: Vec<WindowInfo> = all_windows.iter().filter_map(|w| {
              if w.is_minimized().unwrap_or(false) { return None; }
              let title = w.title().unwrap_or_default();
              if title.is_empty() { return None; }
              let x = w.x().ok()?;
              let y = w.y().ok()?;
              let w_px = w.width().ok()? as i32;
              let h_px = w.height().ok()? as i32;
              if w_px < 1 || h_px < 1 { return None; }
              Some(WindowInfo { title, x, y, w: w_px, h: h_px })
            }).collect();

            thread_state.lock().unwrap().windows = infos;
          }

          std::thread::sleep(std::time::Duration::from_millis(16));
        }
      })
      .expect("failed to spawn window detection thread");

    Self {
      state,
      exclude_prefix: exclude_prefix.to_string(),
    }
  }

  /// Hit-test the cached window list at the given screen coordinates.
  /// Returns the topmost window containing the point.
  pub fn window_at(&self, screen_x: i32, screen_y: i32) -> Option<WindowRect> {
    let s = self.state.lock().unwrap();

    // Window list is z-ordered (topmost first)
    for w in &s.windows {
      if w.title.starts_with(&self.exclude_prefix) {
        continue;
      }

      if screen_x >= w.x && screen_x < w.x + w.w
        && screen_y >= w.y && screen_y < w.y + w.h
      {
        eprintln!("[window-detect] hit: '{}' at ({},{})-({}x{})", w.title, w.x, w.y, w.w, w.h);
        return Some(WindowRect {
          left: w.x,
          top: w.y,
          right: w.x + w.w,
          bottom: w.y + w.h,
        });
      }
    }

    None
  }

  pub fn stop(&self) {
    self.state.lock().unwrap().active = false;
  }
}

impl Drop for WindowDetector {
  fn drop(&mut self) {
    self.stop();
  }
}
