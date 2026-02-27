use std::sync::Mutex;
use tauri::{AppHandle, Emitter, Manager, WebviewUrl, WebviewWindowBuilder};

use crate::capture;
use crate::error::AppError;
use crate::state::AppState;

pub async fn open_overlay(app: &AppHandle) -> Result<(), AppError> {
  // Check if already capturing
  {
    let state = app.state::<Mutex<AppState>>();
    let mut state = state.lock().unwrap();
    if state.is_capturing {
      return Ok(());
    }
    state.is_capturing = true;
  }

  // Capture all monitors
  let screenshot = capture::capture_all_monitors()?;
  let base64_data = capture::image_to_base64(&screenshot)?;

  // Store the screenshot for later cropping
  {
    let state = app.state::<Mutex<AppState>>();
    let mut state = state.lock().unwrap();
    state.pending_screenshot = Some(screenshot);
  }

  // Create the overlay window
  let overlay = WebviewWindowBuilder::new(app, "overlay", WebviewUrl::App("overlay.html".into()))
    .title("QuickShotter Overlay")
    .transparent(true)
    .decorations(false)
    .always_on_top(true)
    .fullscreen(true)
    .focused(true)
    .skip_taskbar(true)
    .build()?;

  // Send the screenshot data to the overlay
  overlay.emit("screenshot-ready", base64_data)?;

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
}
