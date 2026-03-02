use std::sync::Mutex;
use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder};

use crate::capture;
use crate::error::AppError;
use crate::state::{AppState, LockRecover};
use crate::window_capture;

pub fn open_overlay(app: &AppHandle) -> Result<(), AppError> {
  let mode = app.state::<Mutex<AppState>>().lock_or_recover().config.capture_mode.clone();
  open_overlay_with_mode(app, &mode)
}

pub fn open_overlay_with_mode(app: &AppHandle, mode: &str) -> Result<(), AppError> {
  // Atomically check and set is_capturing in a single lock scope
  // to prevent two rapid hotkey presses from opening duplicate overlays,
  // and block captures while the annotation editor is open.
  {
    let s = app.state::<Mutex<AppState>>();
    let mut state = s.lock_or_recover();
    if state.is_capturing || state.is_annotating || state.is_recording {
      return Ok(());
    }
    state.is_capturing = true;
    state.overlay_mode = mode.to_string();
  }

  // Get virtual desktop bounds (fast, no image capture)
  let bounds = match capture::get_desktop_bounds() {
    Ok(b) => b,
    Err(e) => {
      app.state::<Mutex<AppState>>().lock_or_recover().is_capturing = false;
      return Err(e);
    }
  };

  // Start the window detection worker early so it can begin caching results
  // while the overlay window is being created.
  if mode == "window" {
    window_capture::start();
  }

  // Freeze mode captures upfront for a frozen preview.
  // Window mode skips this -- it shows a transparent overlay and captures on click.
  // On macOS, ALL modes pre-capture because hiding the overlay and recapturing
  // has unreliable timing -- the compositor may not finish re-rendering windows
  // within the delay, resulting in only the desktop background being captured.
  let needs_capture = mode == "freeze" || cfg!(target_os = "macos");

  if needs_capture {
    let screen = match capture::capture_all_monitors() {
      Ok(s) => s,
      Err(e) => {
        app.state::<Mutex<AppState>>().lock_or_recover().is_capturing = false;
        return Err(e);
      }
    };

    // Detect likely permission denial (black screenshot) on macOS.
    // Abort early -- continuing would show a useless black overlay.
    #[cfg(target_os = "macos")]
    if capture::is_likely_blank(&screen.image) {
      use tauri_plugin_notification::NotificationExt;
      app.notification()
        .builder()
        .title("Screen Recording Permission Required")
        .body("Grant permission in System Settings > Privacy & Security > Screen Recording, then restart QuickShotter")
        .show()
        .ok();
      app.state::<Mutex<AppState>>().lock_or_recover().is_capturing = false;
      return Ok(());
    }

    // Only generate base64 preview for freeze mode (it displays a frozen image).
    // Instant/window modes on macOS just need the raw image for cropping later.
    let base64_data = if mode == "freeze" {
      match capture::image_to_base64(&screen.image) {
        Ok(d) => Some(d),
        Err(e) => {
          app.state::<Mutex<AppState>>().lock_or_recover().is_capturing = false;
          return Err(e);
        }
      }
    } else {
      None
    };

    let s = app.state::<Mutex<AppState>>();
    let mut state = s.lock_or_recover();
    state.pending_screenshot = Some(screen.image);
    state.pending_base64 = base64_data;
  }

  // Span overlay across the entire virtual desktop (all monitors)
  let build_result = WebviewWindowBuilder::new(app, "overlay", WebviewUrl::App("overlay.html".into()))
    .title("QuickShotter Overlay")
    .transparent(true)
    .decorations(false)
    .shadow(false)
    .always_on_top(true)
    .resizable(false)
    .position(bounds.x as f64, bounds.y as f64)
    .inner_size(bounds.width as f64, bounds.height as f64)
    .visible(false)
    .skip_taskbar(true)
    .build();

  if let Err(e) = build_result {
    // Reset state so future captures aren't permanently blocked
    let s = app.state::<Mutex<AppState>>();
    let mut state = s.lock_or_recover();
    state.is_capturing = false;
    state.pending_screenshot = None;
    state.pending_base64 = None;
    return Err(e.into());
  }

  // Find overlay's xcap window ID for window detection exclusion.
  // Retry because the window may not be visible to xcap immediately after build.
  let mut overlay_id = 0u32;
  for _ in 0..3 {
    overlay_id = xcap::Window::all()
      .unwrap_or_default()
      .iter()
      .find(|w| w.title().unwrap_or_default() == "QuickShotter Overlay")
      .and_then(|w| w.id().ok())
      .unwrap_or(0);
    if overlay_id != 0 { break; }
    std::thread::sleep(std::time::Duration::from_millis(50));
  }
  if overlay_id == 0 {
    eprintln!("Could not find overlay window ID for exclusion");
  }
  app.state::<Mutex<AppState>>().lock_or_recover().overlay_window_id = overlay_id;

  Ok(())
}

pub fn close_overlay(app: &AppHandle) {
  window_capture::stop();
  if let Some(overlay) = app.get_webview_window("overlay") {
    if let Err(e) = overlay.destroy() {
      eprintln!("Failed to destroy overlay window: {e}");
    }
  }
  let s = app.state::<Mutex<AppState>>();
  let mut state = s.lock_or_recover();
  state.is_capturing = false;
  state.pending_screenshot = None;
  state.pending_base64 = None;
  state.overlay_window_id = 0;
}

pub fn open_annotation_window(app: &AppHandle) -> Result<(), AppError> {
  // Use explicit position + size instead of .fullscreen(true) to avoid
  // macOS native fullscreen animation (slides into a new Space).
  let bounds = capture::get_desktop_bounds()?;
  WebviewWindowBuilder::new(app, "annotation", WebviewUrl::App("annotation.html".into()))
    .title("QuickShotter - Annotate")
    .decorations(false)
    .shadow(false)
    .always_on_top(true)
    .resizable(false)
    .position(bounds.x as f64, bounds.y as f64)
    .inner_size(bounds.width as f64, bounds.height as f64)
    .visible(false)
    .skip_taskbar(true)
    .build()?;
  Ok(())
}

pub fn close_annotation_window(app: &AppHandle) {
  if let Some(win) = app.get_webview_window("annotation") {
    if let Err(e) = win.destroy() {
      eprintln!("Failed to destroy annotation window: {e}");
    }
  }
  let s = app.state::<Mutex<AppState>>();
  let mut state = s.lock_or_recover();
  state.pending_annotation = None;
  state.pending_annotation_base64 = None;
  state.is_annotating = false;
}
