use std::sync::Mutex;
use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder};

use crate::capture;
use crate::error::AppError;
use crate::state::AppState;

pub fn open_overlay(app: &AppHandle) -> Result<(), AppError> {
  // Check if already capturing
  {
    let state = app.state::<Mutex<AppState>>();
    let state = state.lock().unwrap();
    if state.is_capturing {
      return Ok(());
    }
  }

  // Get virtual desktop bounds (fast, no image capture)
  let bounds = capture::get_desktop_bounds()?;

  // Read capture mode from config
  let mode = {
    let state = app.state::<Mutex<AppState>>();
    let state = state.lock().unwrap();
    state.config.capture_mode.clone()
  };

  if mode == "freeze" {
    // Freeze mode: capture screen first, show it as overlay background
    let screen = capture::capture_all_monitors()?;
    let base64_data = capture::image_to_base64(&screen.image)?;

    {
      let state = app.state::<Mutex<AppState>>();
      let mut state = state.lock().unwrap();
      state.is_capturing = true;
      state.pending_screenshot = Some(screen.image);
      state.pending_base64 = Some(base64_data);
    }
  } else {
    // Instant mode: no capture, just mark as capturing
    let state = app.state::<Mutex<AppState>>();
    let mut state = state.lock().unwrap();
    state.is_capturing = true;
  }

  // Span overlay across the entire virtual desktop (all monitors)
  WebviewWindowBuilder::new(app, "overlay", WebviewUrl::App("overlay.html".into()))
    .title("QuickShotter Overlay")
    .transparent(true)
    .decorations(false)
    .always_on_top(true)
    .position(bounds.x as f64, bounds.y as f64)
    .inner_size(bounds.width as f64, bounds.height as f64)
    .visible(false)
    .skip_taskbar(true)
    .build()?;

  Ok(())
}

pub fn close_overlay(app: &AppHandle) {
  if let Some(overlay) = app.get_webview_window("overlay") {
    overlay.destroy().ok();
  }
  let state = app.state::<Mutex<AppState>>();
  let mut state = state.lock().unwrap();
  state.is_capturing = false;
  state.pending_screenshot = None;
  state.pending_base64 = None;
}
