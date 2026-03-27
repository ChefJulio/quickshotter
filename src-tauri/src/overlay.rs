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

  // On macOS, check screen recording permission BEFORE capturing or opening
  // the overlay.  Without this gate the overlay and the system permission dialog
  // appear simultaneously, which is confusing.
  #[cfg(target_os = "macos")]
  if !capture::has_screen_recording_permission() {
    capture::request_screen_recording_permission();
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
  // On macOS, screenshot modes pre-capture because hiding the overlay and recapturing
  // has unreliable timing -- the compositor may not finish re-rendering windows
  // within the delay, resulting in only the desktop background being captured.
  // Recording always uses instant mode -- freezing the screen for region selection
  // makes no sense when the goal is to capture live video.
  let is_recording = mode == "record_region";
  let needs_capture = !is_recording && (mode == "freeze" || cfg!(target_os = "macos"));

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
  // Use more retries with longer delays -- on boot the system may be slow.
  let mut overlay_id = 0u32;
  for _ in 0..10 {
    overlay_id = xcap::Window::all()
      .unwrap_or_default()
      .iter()
      .find(|w| w.title().unwrap_or_default() == "QuickShotter Overlay")
      .and_then(|w| w.id().ok())
      .unwrap_or(0);
    if overlay_id != 0 { break; }
    std::thread::sleep(std::time::Duration::from_millis(100));
  }
  if overlay_id == 0 {
    eprintln!("Warning: Could not find overlay window ID for exclusion (window capture exclusion disabled)");
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
  // Size the window to 80% of the primary monitor, centered.
  let (x, y, w, h) = if let Ok(Some(monitor)) = app.primary_monitor() {
    let pos = monitor.position();
    let size = monitor.size();
    let sf = monitor.scale_factor();
    let mon_w = size.width as f64 / sf;
    let mon_h = size.height as f64 / sf;
    let win_w = (mon_w * 0.8).round();
    let win_h = (mon_h * 0.8).round();
    let win_x = pos.x as f64 + (mon_w - win_w) / 2.0;
    let win_y = pos.y as f64 + (mon_h - win_h) / 2.0;
    (win_x, win_y, win_w, win_h)
  } else {
    let bounds = capture::get_desktop_bounds()?;
    let w = (bounds.width as f64 * 0.8).round();
    let h = (bounds.height as f64 * 0.8).round();
    let x = bounds.x as f64 + (bounds.width as f64 - w) / 2.0;
    let y = bounds.y as f64 + (bounds.height as f64 - h) / 2.0;
    (x, y, w, h)
  };
  WebviewWindowBuilder::new(app, "annotation", WebviewUrl::App("annotation.html".into()))
    .title("QuickShotter - Annotate")
    .decorations(true)
    .resizable(true)
    .position(x, y)
    .inner_size(w, h)
    .min_inner_size(400.0, 300.0)
    .visible(false)
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
  state.annotation_source_path = None;
  state.is_annotating = false;
}
