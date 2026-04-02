use image::RgbaImage;
use std::path::PathBuf;
use std::sync::Mutex;
use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder};

use crate::capture;
use crate::config;
use crate::error::AppError;
use crate::hotkeys;
use crate::overlay;
use crate::state::{AppState, LockRecover};
use crate::tray;
use crate::window_capture;

#[derive(serde::Serialize)]
pub struct CaptureResultDto {
  pub filepath: Option<String>,
  pub copied_to_clipboard: bool,
}

#[derive(serde::Serialize)]
pub struct AnnotationConfigDto {
  pub shift_tool: String,
  pub ctrl_tool: String,
  pub alt_tool: String,
  pub default_tool: String,
  pub right_default_tool: String,
  pub right_shift_tool: String,
  pub right_ctrl_tool: String,
  pub right_alt_tool: String,
}

/// Returns "live", "freeze", or "window" so the overlay JS knows which mode.
#[tauri::command]
pub fn get_capture_delay(app: AppHandle) -> u32 {
  app.state::<Mutex<AppState>>().lock_or_recover().config.capture_delay
}

#[tauri::command]
pub fn get_overlay_mode(app: AppHandle) -> String {
  app.state::<Mutex<AppState>>().lock_or_recover().overlay_mode.clone()
}

/// Returns the virtual desktop origin so the overlay JS can convert
/// absolute screen coordinates to overlay-local coordinates.
#[tauri::command]
pub fn get_overlay_origin() -> Result<(i32, i32), AppError> {
  let bounds = capture::get_desktop_bounds()?;
  Ok((bounds.x, bounds.y))
}

/// Request screen recording permission on macOS.
/// CGRequestScreenCaptureAccess adds the app to the list and shows a system
/// dialog on first call. On subsequent calls it returns silently, so we also
/// open Settings directly as a fallback — the user always has somewhere to go.
#[tauri::command]
pub fn request_permission() {
  #[cfg(target_os = "macos")]
  {
    // This adds QuickShotter to the list + shows dialog (first time only)
    capture::request_screen_recording_permission();
    // Always open Settings as fallback (dialog may not appear on repeat calls)
    std::thread::sleep(std::time::Duration::from_millis(300));
    capture::open_screen_recording_settings();
  }
}

/// Open Screen Recording settings directly (no system dialog).
/// Used from the Settings > About section.
#[tauri::command]
pub fn open_permission_settings() {
  #[cfg(target_os = "macos")]
  capture::open_screen_recording_settings();
}

/// Check if screen recording permission is currently granted.
#[tauri::command]
pub fn check_permission() -> bool {
  #[cfg(target_os = "macos")]
  { capture::has_screen_recording_permission() }
  #[cfg(not(target_os = "macos"))]
  { true }
}

/// Mark onboarding as complete and close the welcome window.
#[tauri::command]
pub fn complete_onboarding(app: AppHandle) {
  let config_dir = app.path().app_config_dir().ok();
  if let Some(dir) = config_dir {
    let flag = dir.join(".onboarded");
    let _ = std::fs::create_dir_all(&dir);
    let _ = std::fs::write(flag, "1");
  }
  // Close the welcome window from Rust side
  if let Some(win) = app.get_webview_window("welcome") {
    win.destroy().ok();
  }
}

// -- Shared capture finalization --

/// Copy image to clipboard, save to disk, update history, refresh tray, and
/// send a desktop notification.  Called from every capture path to avoid
/// duplicating this pipeline.
fn finalize_capture(app: &AppHandle, img: &RgbaImage) -> Result<CaptureResultDto, AppError> {
  finalize_capture_with_toggle(app, img, false)
}

/// Like finalize_capture, but toggle_save XORs the config's save_to_disk setting.
/// Alt held during capture toggles: if save is OFF, Alt forces save; if ON, Alt skips.
fn finalize_capture_with_toggle(app: &AppHandle, img: &RgbaImage, toggle_save: bool) -> Result<CaptureResultDto, AppError> {
  let mut config = app.state::<Mutex<AppState>>().lock_or_recover().config.clone();

  // Alt modifier toggles save-to-disk for this capture only
  if toggle_save {
    config.save_to_disk = !config.save_to_disk;
  }

  // Clipboard copy is synchronous — user needs to paste immediately.
  let t_fin = std::time::Instant::now();
  let clipboard_action = config.clipboard_action.clone();
  let copied = if clipboard_action == "image" {
    capture::copy_to_clipboard(img)?;
    eprintln!("[timing] clipboard copy ({}x{}): {:?}", img.width(), img.height(), t_fin.elapsed());
    true
  } else {
    false
  };

  // Disk save + upload run in the background — no reason to block the return.
  // Pre-generate the filepath so we can return it immediately and update history.
  let filepath = if config.save_to_disk {
    capture::reserve_filepath(&config)?
  } else {
    None
  };
  let filepath_str = filepath.as_ref().map(|p: &PathBuf| p.to_string_lossy().to_string());

  if let Some(ref path) = filepath {
    let s = app.state::<Mutex<AppState>>();
    let mut state = s.lock_or_recover();
    state.add_to_history(path.clone());
    state.last_saved_path = Some(path.clone());
  }

  // Fire off background work: disk save, upload, tray refresh, notification
  let img_bg = img.clone();
  let app_bg = app.clone();
  let filepath_bg = filepath.clone();
  let filepath_str_bg = filepath_str.clone();
  std::thread::spawn(move || {
    // Save to disk
    if let Some(ref path) = filepath_bg {
      let t_save = std::time::Instant::now();
      if let Err(e) = capture::write_image_to_path(&img_bg, path, &config) {
        eprintln!("Background save failed: {e}");
      } else {
        eprintln!("[timing] disk save ({} format): {:?}", config.format, t_save.elapsed());
      }
    }

    // Upload if clipboard_action is "url"
    if clipboard_action == "url" {
      let app2 = app_bg.clone();
      tauri::async_runtime::spawn(async move {
        match crate::catbox::upload(&img_bg).await {
          Ok(url) => {
            capture::copy_text_to_clipboard(&url).ok();
            use tauri_plugin_notification::NotificationExt;
            app2.notification()
              .builder()
              .title("Uploaded to catbox.moe")
              .body(&url)
              .show()
              .ok();
            #[cfg(target_os = "windows")]
            { std::process::Command::new("cmd").args(["/c", "start", &url]).spawn().ok(); }
            #[cfg(target_os = "macos")]
            { std::process::Command::new("open").arg(&url).spawn().ok(); }
          }
          Err(e) => {
            eprintln!("Upload failed: {e}");
            use tauri_plugin_notification::NotificationExt;
            app2.notification()
              .builder()
              .title("Upload failed")
              .body(&e.to_string())
              .show()
              .ok();
          }
        }
      });
    }

    tray::refresh_tray_menu(&app_bg);
    notify_capture(&app_bg, filepath_str_bg.as_deref());
  });

  Ok(CaptureResultDto {
    filepath: filepath_str,
    copied_to_clipboard: copied,
  })
}


/// Prepare a delayed capture: save params, close overlay, open countdown window.
/// The countdown window self-drives its timer and calls execute_delayed_capture when done.
#[tauri::command]
pub async fn prepare_delayed_capture(
  app: AppHandle,
  params: serde_json::Value,
  pos_x: f64,
  pos_y: f64,
  sel_x: Option<f64>,
  sel_y: Option<f64>,
  sel_w: Option<f64>,
  sel_h: Option<f64>,
) -> Result<(), AppError> {
  // Save capture params
  app.state::<Mutex<AppState>>().lock_or_recover().delayed_capture = Some(params);

  // Close the overlay so the user can interact with their desktop
  overlay::close_overlay(&app);

  // Clean up any leftover countdown window from a previous capture
  if let Some(w) = app.get_webview_window("countdown") {
    w.destroy().ok();
  }

  // Brief pause for overlay to de-render
  let _ = tauri::async_runtime::spawn_blocking(|| {
    std::thread::sleep(std::time::Duration::from_millis(50));
  }).await;

  // All positions are now in physical screen coordinates (converted by JS).
  // Use the coords module to convert to Tauri logical for .position()/.inner_size().
  use crate::coords::{self, ScreenPoint, ScreenSize};

  // Register a temporary global ESC shortcut to cancel the countdown.
  {
    use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut};
    let esc: Shortcut = "Escape".parse().unwrap();
    let app2 = app.clone();
    app.global_shortcut().on_shortcut(esc, move |_app, _shortcut, _event| {
      cancel_delayed_capture(app2.clone());
    }).ok();
  }

  // Position countdown window — clamp to the target monitor so it
  // doesn't spill onto an adjacent screen.
  let countdown_phys = ScreenPoint { x: pos_x, y: pos_y };
  let countdown_size = ScreenSize { w: 80.0, h: 80.0 };
  let (countdown_phys, countdown_size) = coords::clamp_to_monitor(
    countdown_phys, countdown_size, countdown_phys, &app,
  );
  let (cx, cy) = coords::to_tauri_pos(countdown_phys, &app);
  let (cw, ch) = coords::to_tauri_size(countdown_size, countdown_phys, &app);

  use tauri::{WebviewUrl, WebviewWindowBuilder};
  WebviewWindowBuilder::new(&app, "countdown", WebviewUrl::App("countdown.html".into()))
    .title("Countdown")
    .transparent(true)
    .decorations(false)
    .shadow(false)
    .always_on_top(true)
    .resizable(false)
    .inner_size(cw, ch)
    .position(cx, cy)
    .skip_taskbar(true)
    .build()
    .map_err(|e| AppError::Capture(format!("Failed to open countdown: {e}")))?;

  // Selection border — clamp to the monitor containing the selection center.
  if let (Some(sx), Some(sy), Some(sw), Some(sh)) = (sel_x, sel_y, sel_w, sel_h) {
    if sw > 0.0 && sh > 0.0 {
      let pad = 3.0;
      let border_pos = ScreenPoint { x: sx - pad, y: sy - pad };
      let border_size = ScreenSize { w: sw + pad * 2.0, h: sh + pad * 2.0 };
      let sel_center = ScreenPoint { x: sx + sw / 2.0, y: sy + sh / 2.0 };
      let (bp, bs) = coords::clamp_to_monitor(border_pos, border_size, sel_center, &app);
      let (bx, by) = coords::to_tauri_pos(bp, &app);
      let (bw, bh) = coords::to_tauri_size(bs, bp, &app);

      WebviewWindowBuilder::new(&app, "delay-border", WebviewUrl::App("delay-border.html".into()))
        .title("Selection")
        .transparent(true)
        .decorations(false)
        .shadow(false)
        .always_on_top(true)
        .resizable(false)
        .inner_size(bw, bh)
        .position(bx, by)
        .skip_taskbar(true)
        .build()
        .ok();
    }
  }

  Ok(())
}

/// Cancel a delayed capture (e.g. user pressed ESC during countdown).
#[tauri::command]
pub fn cancel_delayed_capture(app: AppHandle) {
  app.state::<Mutex<AppState>>().lock_or_recover().delayed_capture = None;
  if let Some(w) = app.get_webview_window("countdown") {
    w.hide().ok();
    w.destroy().ok();
  }
  if let Some(w) = app.get_webview_window("delay-border") {
    w.hide().ok();
    w.destroy().ok();
  }
  // Unregister ESC and re-register all hotkeys on a deferred task
  // (can't safely unregister_all from inside a shortcut handler)
  let app2 = app.clone();
  tauri::async_runtime::spawn(async move {
    // Small delay to let the current shortcut handler finish
    let _ = tauri::async_runtime::spawn_blocking(|| {
      std::thread::sleep(std::time::Duration::from_millis(50));
    }).await;
    crate::hotkeys::unregister_all(&app2);
    if let Err(e) = crate::hotkeys::register_hotkeys(&app2) {
      eprintln!("hotkey re-register failed: {e}");
    }
  });
}

/// Execute a delayed capture using previously saved params. Called by the countdown window.
#[tauri::command]
pub async fn execute_delayed_capture(app: AppHandle) -> Result<(), AppError> {
  // Hide countdown and border windows, wait for de-render, then destroy.
  // hide() is instant and skips the Windows close animation.
  if let Some(w) = app.get_webview_window("countdown") {
    w.hide().ok();
  }
  if let Some(w) = app.get_webview_window("delay-border") {
    w.hide().ok();
  }
  // Wait for windows to fully disappear from screen
  let _ = tauri::async_runtime::spawn_blocking(|| {
    std::thread::sleep(std::time::Duration::from_millis(50));
  }).await;
  // Now destroy (after screenshot area is clear)
  if let Some(w) = app.get_webview_window("countdown") {
    w.destroy().ok();
  }
  if let Some(w) = app.get_webview_window("delay-border") {
    w.destroy().ok();
  }
  // Unregister the temporary ESC shortcut
  {
    use tauri_plugin_global_shortcut::GlobalShortcutExt;
    if let Ok(esc) = "Escape".parse::<tauri_plugin_global_shortcut::Shortcut>() {
      app.global_shortcut().unregister(esc).ok();
    }
  }

  let params = app.state::<Mutex<AppState>>().lock_or_recover().delayed_capture.take();
  let params = match params {
    Some(p) => p,
    None => return Ok(()),
  };

  let mode = params["mode"].as_str().unwrap_or("");

  match mode {
    "region" => {
      let x1 = params["x1"].as_u64().unwrap_or(0) as u32;
      let y1 = params["y1"].as_u64().unwrap_or(0) as u32;
      let x2 = params["x2"].as_u64().unwrap_or(0) as u32;
      let y2 = params["y2"].as_u64().unwrap_or(0) as u32;
      let force_annotate = params["forceAnnotate"].as_bool().unwrap_or(false);
      let toggle_save = params["toggleSave"].as_bool().unwrap_or(false);
      complete_region_capture(app, x1, y1, x2, y2, Some(force_annotate), Some(toggle_save)).await?;
    }
    "window" => {
      let left = params["left"].as_i64().unwrap_or(0) as i32;
      let top = params["top"].as_i64().unwrap_or(0) as i32;
      let right = params["right"].as_i64().unwrap_or(0) as i32;
      let bottom = params["bottom"].as_i64().unwrap_or(0) as i32;
      let force_annotate = params["forceAnnotate"].as_bool().unwrap_or(false);
      let toggle_save = params["toggleSave"].as_bool().unwrap_or(false);
      complete_window_capture(app, left, top, right, bottom, Some(force_annotate), Some(toggle_save)).await?;
    }
    "fullscreen" => {
      do_fullscreen_capture(&app).await?;
    }
    "ocr" => {
      let x1 = params["x1"].as_u64().unwrap_or(0) as u32;
      let y1 = params["y1"].as_u64().unwrap_or(0) as u32;
      let x2 = params["x2"].as_u64().unwrap_or(0) as u32;
      let y2 = params["y2"].as_u64().unwrap_or(0) as u32;
      complete_ocr_capture(app, x1, y1, x2, y2).await?;
    }
    "select_screen" => {
      let idx = params["monitorIndex"].as_u64().unwrap_or(0) as usize;
      complete_select_screen_capture(app, idx).await?;
    }
    _ => {}
  }

  Ok(())
}

/// Delayed capture entry points -- used by both hotkey and tray handlers.
/// Centralizes the delay-then-capture pattern so callers don't duplicate it.

pub fn delayed_region_capture(app: &AppHandle) {
  // No pre-overlay delay for region -- delay happens after selection
  if let Err(e) = overlay::open_overlay(app) {
    eprintln!("Region capture failed: {e}");
  }
}

pub fn delayed_window_capture(app: &AppHandle) {
  // No pre-overlay delay for window -- delay happens after selection
  if let Err(e) = overlay::open_overlay_with_mode(app, "window") {
    eprintln!("Window capture failed: {e}");
  }
}

pub fn delayed_fullscreen_capture(app: &AppHandle) {
  let (mode, delay) = {
    let s = app.state::<Mutex<AppState>>();
    let state = s.lock_or_recover();
    (state.config.fullscreen_mode.clone(), state.config.capture_delay)
  };

  if mode == "select" {
    // Select screen uses overlay — delay handled in captureWithDelay on frontend
    if let Err(e) = overlay::open_overlay_with_mode(app, "select_screen") {
      eprintln!("Select screen overlay failed: {e}");
    }
    return;
  }

  if delay > 0 {
    // Prevent double-fire: if a delayed capture is already pending, ignore
    if app.state::<Mutex<AppState>>().lock_or_recover().delayed_capture.is_some() {
      return;
    }

    // Clean up any leftover countdown from a previous capture
    if let Some(w) = app.get_webview_window("countdown") { w.hide().ok(); w.destroy().ok(); }
    if let Some(w) = app.get_webview_window("delay-border") { w.hide().ok(); w.destroy().ok(); }

    // Detect which monitor the cursor is on NOW (before the countdown runs)
    // so "current screen" captures the right one even if the user moves the cursor.
    // Coordinates are physical screen pixels — prepare_delayed_capture expects physical.
    let (cursor_monitor_idx, countdown_x, countdown_y) = {
      let monitors = xcap::Monitor::all().unwrap_or_default();
      let (cx, cy) = capture::get_cursor_position_public();
      let idx = monitors.iter().position(|m| {
        let Ok(mx) = m.x() else { return false };
        let Ok(my) = m.y() else { return false };
        let Ok(mw) = m.width() else { return false };
        let Ok(mh) = m.height() else { return false };
        cx >= mx && cx < mx + mw as i32 && cy >= my && cy < my + mh as i32
      }).unwrap_or(0);
      let (px, py) = if let Some(m) = monitors.get(idx) {
        (m.x().unwrap_or(0) as f64 + 16.0, m.y().unwrap_or(0) as f64 + 16.0)
      } else {
        (16.0, 16.0)
      };
      (idx, px, py)
    };

    let app = app.clone();
    tauri::async_runtime::spawn(async move {
      // For "current" mode, save the specific monitor index so it captures
      // the right screen even if the cursor moves during countdown.
      let params = if mode == "current" {
        serde_json::json!({ "mode": "select_screen", "monitorIndex": cursor_monitor_idx })
      } else {
        serde_json::json!({ "mode": "fullscreen" })
      };

      if let Err(e) = prepare_delayed_capture(app, params, countdown_x, countdown_y, None, None, None, None).await {
        eprintln!("Fullscreen delayed capture failed: {e}");
      }
    });
  } else {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
      if let Err(e) = do_fullscreen_capture(&app).await {
        eprintln!("Fullscreen capture failed: {e}");
      }
    });
  }
}

pub fn delayed_ocr_capture(app: &AppHandle) {
  // No pre-overlay delay for OCR -- delay happens after selection
  if let Err(e) = overlay::open_overlay_with_mode(app, "ocr") {
    eprintln!("OCR overlay failed: {e}");
  }
}

// -- Capture commands --

#[tauri::command]
pub fn trigger_region_capture(app: AppHandle) -> Result<(), AppError> {
  overlay::open_overlay(&app)
}

#[tauri::command]
pub async fn trigger_fullscreen_capture(app: AppHandle) -> Result<CaptureResultDto, AppError> {
  do_fullscreen_capture(&app).await
}

#[tauri::command]
pub fn trigger_window_capture(app: AppHandle) -> Result<(), AppError> {
  overlay::open_overlay_with_mode(&app, "window")
}

pub async fn do_fullscreen_capture(app: &AppHandle) -> Result<CaptureResultDto, AppError> {
  // Guard against duplicate captures and concurrent annotation
  {
    let s = app.state::<Mutex<AppState>>();
    let mut state = s.lock_or_recover();
    if state.is_capturing || state.is_annotating || state.is_recording {
      return Ok(CaptureResultDto { filepath: None, copied_to_clipboard: false });
    }
    state.is_capturing = true;
  }

  let result = do_fullscreen_capture_inner(app).await;

  // Always reset is_capturing, even on error (annotation sets its own flag)
  app.state::<Mutex<AppState>>().lock_or_recover().is_capturing = false;

  result
}

async fn do_fullscreen_capture_inner(app: &AppHandle) -> Result<CaptureResultDto, AppError> {
  #[cfg(target_os = "macos")]
  if !capture::ensure_screen_recording_permission(app) {
    return Ok(CaptureResultDto { filepath: None, copied_to_clipboard: false });
  }

  let t0 = std::time::Instant::now();
  let fullscreen_mode = app.state::<Mutex<AppState>>().lock_or_recover().config.fullscreen_mode.clone();
  let screen = match fullscreen_mode.as_str() {
    "current" => capture::capture_monitor_at_cursor()?,
    _ => capture::capture_all_monitors()?,
  };
  eprintln!("[timing] fullscreen BitBlt ({}x{}, mode={}): {:?}", screen.width, screen.height, fullscreen_mode, t0.elapsed());

  // Fallback: detect blank screenshot in case permission check passed but
  // capture still returned empty (can happen after app updates on some macOS versions).
  #[cfg(target_os = "macos")]
  if capture::is_likely_blank(&screen.image) {
    capture::notify_blank_capture(app);
    return Ok(CaptureResultDto { filepath: None, copied_to_clipboard: false });
  }

  let annotate = app.state::<Mutex<AppState>>().lock_or_recover().config.annotate_captures;

  if annotate {
    return open_annotation_for_image(app, screen.image).await;
  }

  finalize_capture(app, &screen.image)
}

#[tauri::command]
pub async fn complete_region_capture(
  app: AppHandle,
  x1: u32,
  y1: u32,
  x2: u32,
  y2: u32,
  force_annotate: Option<bool>,
  toggle_save: Option<bool>,
) -> Result<CaptureResultDto, AppError> {
  let force_annotate = force_annotate.unwrap_or(false);
  let toggle_save = toggle_save.unwrap_or(false);

  // Hide overlay immediately (don't destroy -- we're still inside its webview command)
  if let Some(overlay) = app.get_webview_window("overlay") {
    overlay.hide().ok();
  }

  let result = complete_region_capture_inner(&app, x1, y1, x2, y2, force_annotate, toggle_save).await;

  // Always close overlay to avoid leaking the window.
  // Safe to call even if inner function already closed it (idempotent).
  let app2 = app.clone();
  tauri::async_runtime::spawn(async move { overlay::close_overlay(&app2); });

  result
}

async fn complete_region_capture_inner(
  app: &AppHandle,
  x1: u32,
  y1: u32,
  x2: u32,
  y2: u32,
  force_annotate: bool,
  toggle_save: bool,
) -> Result<CaptureResultDto, AppError> {
  let t0 = std::time::Instant::now();

  let left = x1.min(x2);
  let top = y1.min(y2);
  let right = x1.max(x2);
  let bottom = y1.max(y2);
  let w = right - left;
  let h = bottom - top;

  if w < 3 || h < 3 {
    return Err(AppError::Capture("Selection too small".to_string()));
  }

  // Use pre-captured image if available (freeze mode crops from the pre-capture).
  let has_pending = app.state::<Mutex<AppState>>().lock_or_recover().pending_screenshot.is_some();

  // Coordinates here are image-space (already converted from screen-space by the caller).
  let image = if has_pending {
    let s = app.state::<Mutex<AppState>>();
    let state = s.lock_or_recover();
    let screenshot = state
      .pending_screenshot
      .as_ref()
      .ok_or_else(|| AppError::Capture("No pending screenshot".to_string()))?;
    let img = safe_crop(screenshot, left, top, w, h)?;
    eprintln!("[timing] crop from pending: {:?}", t0.elapsed());
    img
  } else {
    // The native overlay daemon already waits one compositor frame (16ms on macOS,
    // DwmFlush on Windows) before sending its result, so the window is already gone.
    // No additional sleep needed.
    eprintln!("[timing] compositor wait: {:?}", t0.elapsed());
    // Capture just the selected region via BitBlt — much faster than
    // capturing the full desktop and cropping.
    let img = capture::capture_region(left as i32, top as i32, w, h)?;
    eprintln!("[timing] region capture: {:?}", t0.elapsed());
    img
  };

  eprintln!("[timing] image ready ({}x{}): {:?}", image.width(), image.height(), t0.elapsed());

  // Check if we should open annotation editor (config setting or shift-key override)
  let annotate = force_annotate
    || app.state::<Mutex<AppState>>().lock_or_recover().config.annotate_captures;

  if annotate {
    return open_annotation_for_image(app, image).await;
  }

  let result = finalize_capture_with_toggle(app, &image, toggle_save);
  eprintln!("[timing] *** TOTAL mouse-up → clipboard: {:?} ***", t0.elapsed());
  result
}

// -- Native overlay dispatch (called from overlay daemon result handler) --

/// Called by the native overlay daemon when a region is selected.
/// Wraps the existing complete_region_capture_inner with the overlay-specific flow.
pub async fn complete_region_capture_from_overlay(
  app: &AppHandle,
  x1: i32, y1: i32, x2: i32, y2: i32,
  shift: bool, alt: bool,
) -> Result<CaptureResultDto, AppError> {
  let has_pending = app.state::<Mutex<AppState>>().lock_or_recover().pending_screenshot.is_some();

  if has_pending {
    // Freeze mode: crop from pre-captured image using image-space coords
    let (bx, by) = {
      let s = app.state::<Mutex<AppState>>();
      let state = s.lock_or_recover();
      match state.cached_bounds {
        Some((x, y, _, _)) => (x, y),
        None => {
          drop(state);
          let b = capture::get_desktop_bounds()?;
          (b.x, b.y)
        }
      }
    };
    let img_x1 = ((x1.min(x2) - bx).max(0)) as u32;
    let img_y1 = ((y1.min(y2) - by).max(0)) as u32;
    let img_x2 = ((x1.max(x2) - bx).max(0)) as u32;
    let img_y2 = ((y1.max(y2) - by).max(0)) as u32;
    complete_region_capture_inner(app, img_x1, img_y1, img_x2, img_y2, shift, alt).await
  } else {
    // Live mode: use screen coords directly — capture_region uses BitBlt
    // which operates in screen space. Pass raw screen coords as-is.
    let sx = x1.min(x2);
    let sy = y1.min(y2);
    let sw = (x1.max(x2) - sx) as u32;
    let sh = (y1.max(y2) - sy) as u32;
    complete_region_capture_live(app, sx, sy, sw, sh, shift, alt).await
  }
}

/// Live mode region capture — uses BitBlt to capture just the selected region.
/// Screen coords are absolute (from daemon). No full-desktop capture needed.
async fn complete_region_capture_live(
  app: &AppHandle,
  screen_x: i32, screen_y: i32, w: u32, h: u32,
  force_annotate: bool, toggle_save: bool,
) -> Result<CaptureResultDto, AppError> {
  let t0 = std::time::Instant::now();

  if w < 3 || h < 3 {
    return Err(AppError::Capture("Selection too small".to_string()));
  }

  // No compositor wait needed — the daemon called DwmFlush() before sending
  // the result, guaranteeing the overlay window is fully removed.

  // Capture just the selected region via BitBlt
  let image = capture::capture_region(screen_x, screen_y, w, h)?;
  eprintln!("[timing] region BitBlt ({}x{}): {:?}", w, h, t0.elapsed());

  let annotate = force_annotate
    || app.state::<Mutex<AppState>>().lock_or_recover().config.annotate_captures;

  if annotate {
    return open_annotation_for_image(app, image).await;
  }

  let result = finalize_capture_with_toggle(app, &image, toggle_save);
  eprintln!("[timing] finalize done: {:?}", t0.elapsed());
  result
}

/// Called by the native overlay daemon when a window is selected.
pub async fn complete_window_capture_from_overlay(
  app: &AppHandle,
  left: i32, top: i32, right: i32, bottom: i32,
  shift: bool, alt: bool,
) -> Result<CaptureResultDto, AppError> {
  let has_pending = app.state::<Mutex<AppState>>().lock_or_recover().pending_screenshot.is_some();

  if !has_pending {
    // Live mode: BitBlt the window bounds directly
    let w = (right - left).max(0) as u32;
    let h = (bottom - top).max(0) as u32;
    return complete_region_capture_live(app, left, top, w, h, shift, alt).await;
  }

  complete_window_capture_inner(app, left, top, right, bottom, shift, alt).await
}

// -- OCR capture command --

#[tauri::command]
pub async fn complete_ocr_capture(
  app: AppHandle,
  x1: u32,
  y1: u32,
  x2: u32,
  y2: u32,
) -> Result<String, AppError> {
  // Hide overlay immediately
  if let Some(overlay) = app.get_webview_window("overlay") {
    overlay.hide().ok();
  }

  let result = complete_ocr_capture_inner(&app, x1, y1, x2, y2).await;

  let app2 = app.clone();
  tauri::async_runtime::spawn(async move { overlay::close_overlay(&app2); });

  result
}

/// Called by the native overlay daemon when OCR region is selected.
pub async fn complete_ocr_capture_from_overlay(
  app: &AppHandle,
  x1: u32, y1: u32, x2: u32, y2: u32,
) -> Result<String, AppError> {
  complete_ocr_capture_inner(app, x1, y1, x2, y2).await
}

async fn complete_ocr_capture_inner(
  app: &AppHandle,
  x1: u32,
  y1: u32,
  x2: u32,
  y2: u32,
) -> Result<String, AppError> {
  let left = x1.min(x2);
  let top = y1.min(y2);
  let right = x1.max(x2);
  let bottom = y1.max(y2);
  let w = right - left;
  let h = bottom - top;

  if w < 3 || h < 3 {
    return Err(AppError::Ocr("Selection too small".to_string()));
  }

  // Get the cropped image (same logic as region capture)
  let has_pending = app.state::<Mutex<AppState>>().lock_or_recover().pending_screenshot.is_some();

  let image = if has_pending {
    let s = app.state::<Mutex<AppState>>();
    let state = s.lock_or_recover();
    let screenshot = state
      .pending_screenshot
      .as_ref()
      .ok_or_else(|| AppError::Ocr("No pending screenshot".to_string()))?;
    safe_crop(screenshot, left, top, w, h)?
  } else {
    let _ = tauri::async_runtime::spawn_blocking(|| {
      std::thread::sleep(std::time::Duration::from_millis(50));
    }).await;
    let screen = capture::capture_all_monitors()?;
    safe_crop(&screen.image, left, top, w, h)?
  };

  // Run OCR on a blocking thread (WinRT async / Vision sync both block)
  let text = tauri::async_runtime::spawn_blocking(move || {
    crate::ocr::recognize_text(&image)
  }).await.map_err(|e| AppError::Ocr(format!("OCR task panicked: {e}")))?
    ?;

  if text.is_empty() {
    use tauri_plugin_notification::NotificationExt;
    app.notification()
      .builder()
      .title("OCR")
      .body("No text recognized")
      .show()
      .ok();
    return Ok(String::new());
  }

  // Copy recognized text to clipboard
  capture::copy_text_to_clipboard(&text)?;

  // Notify with a preview of the recognized text
  use tauri_plugin_notification::NotificationExt;
  let preview: String = text.chars().take(80).collect();
  let body = if text.len() > 80 {
    format!("{}...", preview)
  } else {
    preview
  };
  app.notification()
    .builder()
    .title("Text copied to clipboard")
    .body(&body)
    .show()
    .ok();

  Ok(text)
}

// -- OCR from image data (used by annotation editor grab-text tool) --

#[tauri::command]
pub async fn ocr_image(
  app: AppHandle,
  image_base64: String,
) -> Result<String, AppError> {
  let png_bytes = base64::Engine::decode(
    &base64::engine::general_purpose::STANDARD,
    &image_base64,
  )
  .map_err(|e| AppError::Ocr(format!("Failed to decode base64: {e}")))?;

  let img = image::load_from_memory(&png_bytes)
    .map_err(|e| AppError::Ocr(format!("Failed to decode image: {e}")))?
    .to_rgba8();

  let text = tauri::async_runtime::spawn_blocking(move || {
    crate::ocr::recognize_text(&img)
  }).await.map_err(|e| AppError::Ocr(format!("OCR task panicked: {e}")))?
    ?;

  if text.is_empty() {
    use tauri_plugin_notification::NotificationExt;
    app.notification()
      .builder()
      .title("OCR")
      .body("No text recognized")
      .show()
      .ok();
    return Ok(String::new());
  }

  capture::copy_text_to_clipboard(&text)?;

  use tauri_plugin_notification::NotificationExt;
  let preview: String = text.chars().take(80).collect();
  let body = if text.len() > 80 {
    format!("{}...", preview)
  } else {
    preview
  };
  app.notification()
    .builder()
    .title("Text copied to clipboard")
    .body(&body)
    .show()
    .ok();

  Ok(text)
}

// -- Window capture commands --

#[tauri::command]
pub fn get_window_at_cursor(app: AppHandle) -> Option<window_capture::WindowRect> {
  let exclude_id = app.state::<Mutex<AppState>>().lock_or_recover().overlay_window_id;
  let (cx, cy) = window_capture::get_cursor_pos()?;
  window_capture::get_window_rect_at(cx, cy, exclude_id)
}

#[tauri::command]
pub async fn complete_window_capture(
  app: AppHandle,
  left: i32,
  top: i32,
  right: i32,
  bottom: i32,
  force_annotate: Option<bool>,
  toggle_save: Option<bool>,
) -> Result<CaptureResultDto, AppError> {
  let force_annotate = force_annotate.unwrap_or(false);
  let toggle_save = toggle_save.unwrap_or(false);

  // Hide overlay immediately
  if let Some(overlay) = app.get_webview_window("overlay") {
    overlay.hide().ok();
  }

  let result = complete_window_capture_inner(&app, left, top, right, bottom, force_annotate, toggle_save).await;

  // Always close overlay to avoid leaking the window.
  // Safe to call even if already closed (idempotent).
  let app2 = app.clone();
  tauri::async_runtime::spawn(async move { overlay::close_overlay(&app2); });

  result
}

async fn complete_window_capture_inner(
  app: &AppHandle,
  left: i32,
  top: i32,
  right: i32,
  bottom: i32,
  force_annotate: bool,
  toggle_save: bool,
) -> Result<CaptureResultDto, AppError> {
  if right <= left || bottom <= top {
    return Err(AppError::Capture("Invalid window bounds".to_string()));
  }

  let w = (right - left) as u32;
  let h = (bottom - top) as u32;

  if w < 3 || h < 3 {
    return Err(AppError::Capture("Window too small".to_string()));
  }

  // Use pre-captured image if available (on macOS, all modes pre-capture to
  // avoid timing issues with overlay compositing after hide).
  let has_pending = app.state::<Mutex<AppState>>().lock_or_recover().pending_screenshot.is_some();

  // xcap window coordinates are in logical points on macOS but physical pixels
  // on Windows. The captured image is always in physical pixels. Compute the
  // scale factor so we crop at the correct physical pixel offsets.
  // Use cached bounds from overlay open when available.
  let bounds = {
    let s = app.state::<Mutex<AppState>>();
    let state = s.lock_or_recover();
    match state.cached_bounds {
      Some((x, y, w, h)) => capture::DesktopBounds { x, y, width: w, height: h },
      None => {
        drop(state);
        capture::get_desktop_bounds()?
      }
    }
  };

  let image = if has_pending {
    let s = app.state::<Mutex<AppState>>();
    let state = s.lock_or_recover();
    let screenshot = state
      .pending_screenshot
      .as_ref()
      .ok_or_else(|| AppError::Capture("No pending screenshot".to_string()))?;
    let sx = screenshot.width() as f64 / bounds.width.max(1) as f64;
    let sy = screenshot.height() as f64 / bounds.height.max(1) as f64;
    let crop_x = ((left - bounds.x) as f64 * sx).max(0.0) as u32;
    let crop_y = ((top - bounds.y) as f64 * sy).max(0.0) as u32;
    let w = ((right - left) as f64 * sx).max(1.0) as u32;
    let h = ((bottom - top) as f64 * sy).max(1.0) as u32;
    safe_crop(screenshot, crop_x, crop_y, w, h)?
  } else {
    // Capture fresh after overlay is hidden (outer function already hid it)
    let _ = tauri::async_runtime::spawn_blocking(|| {
      std::thread::sleep(std::time::Duration::from_millis(50));
    }).await;
    let screen = capture::capture_all_monitors()?;
    let sx = screen.image.width() as f64 / bounds.width.max(1) as f64;
    let sy = screen.image.height() as f64 / bounds.height.max(1) as f64;
    let crop_x = ((left - screen.origin_x) as f64 * sx).max(0.0) as u32;
    let crop_y = ((top - screen.origin_y) as f64 * sy).max(0.0) as u32;
    let w = ((right - left) as f64 * sx).max(1.0) as u32;
    let h = ((bottom - top) as f64 * sy).max(1.0) as u32;
    safe_crop(&screen.image, crop_x, crop_y, w, h)?
  };

  // Check if we should open annotation editor (config setting or shift-key override)
  let annotate = force_annotate
    || app.state::<Mutex<AppState>>().lock_or_recover().config.annotate_captures;

  if annotate {
    return open_annotation_for_image(app, image).await;
  }

  finalize_capture_with_toggle(app, &image, toggle_save)
}

// -- Monitor selection commands --

#[tauri::command]
pub fn get_monitor_list() -> Result<Vec<capture::MonitorInfo>, AppError> {
  capture::get_monitor_info()
}

#[tauri::command]
pub async fn complete_select_screen_capture(
  app: AppHandle,
  monitor_index: usize,
) -> Result<CaptureResultDto, AppError> {
  // Hide overlay first (webview path — daemon path already closed it)
  if let Some(overlay) = app.get_webview_window("overlay") {
    overlay.hide().ok();
  }

  // No compositor sleep needed — daemon calls DwmFlush before sending result.
  // capture_monitor uses BitBlt on Windows (fast).
  let t0 = std::time::Instant::now();
  let screen = capture::capture_monitor(monitor_index)?;
  eprintln!("[timing] monitor BitBlt ({}x{}): {:?}", screen.width, screen.height, t0.elapsed());

  // Close overlay
  overlay::close_overlay(&app);

  let annotate = app.state::<Mutex<AppState>>().lock_or_recover().config.annotate_captures;
  if annotate {
    return open_annotation_for_image(&app, screen.image).await;
  }

  finalize_capture(&app, &screen.image)
}

// -- Annotation commands --

/// Store image for annotation and open the editor window.
/// Sets `is_annotating` to block concurrent captures while the editor is open.
/// Writes image to a temp JPEG file loaded via asset protocol (no base64 IPC).
async fn open_annotation_for_image(
  app: &AppHandle,
  image: image::RgbaImage,
) -> Result<CaptureResultDto, AppError> {
  // Write JPEG to temp file — ~10x faster than PNG encode + base64 + IPC transfer.
  // The original RgbaImage stays in pending_annotation for lossless final save.
  let temp_path = std::env::temp_dir().join("qs_annotation.jpg");
  {
    let rgb = capture::rgba_to_rgb_pub(&image)?;
    let file = std::fs::File::create(&temp_path)
      .map_err(|e| AppError::Annotation(format!("Failed to create temp file: {e}")))?;
    let mut writer = std::io::BufWriter::new(file);
    let mut encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut writer, 90);
    encoder
      .encode(&rgb, rgb.width(), rgb.height(), image::ExtendedColorType::Rgb8)
      .map_err(|e| AppError::Annotation(format!("Failed to encode temp JPEG: {e}")))?;
  }

  {
    let s = app.state::<Mutex<AppState>>();
    let mut state = s.lock_or_recover();
    state.pending_annotation = Some(image);
    state.pending_annotation_path = Some(temp_path);
    state.is_annotating = true;
  }

  if let Err(e) = overlay::open_annotation_window(app) {
    // Reset annotation state so future captures aren't permanently blocked
    let s = app.state::<Mutex<AppState>>();
    let mut state = s.lock_or_recover();
    state.is_annotating = false;
    state.pending_annotation = None;
    state.pending_annotation_path = None;
    return Err(e);
  }

  // Return a placeholder -- the actual save happens from the annotation editor
  Ok(CaptureResultDto {
    filepath: None,
    copied_to_clipboard: false,
  })
}

/// Annotation editor: fetch the temp file path for the annotation image.
/// The frontend loads this via Tauri's asset protocol (convertFileSrc).
#[tauri::command]
pub fn get_pending_annotation(app: AppHandle) -> Result<String, AppError> {
  app.state::<Mutex<AppState>>().lock_or_recover()
    .pending_annotation_path
    .as_ref()
    .map(|p| p.to_string_lossy().to_string())
    .ok_or_else(|| AppError::Annotation("No pending annotation image".to_string()))
}

/// Annotation editor: fetch modifier-to-tool config.
#[tauri::command]
pub fn get_annotation_config(app: AppHandle) -> AnnotationConfigDto {
  let s = app.state::<Mutex<AppState>>();
  let state = s.lock_or_recover();
  AnnotationConfigDto {
    shift_tool: state.config.annotate_shift_tool.clone(),
    ctrl_tool: state.config.annotate_ctrl_tool.clone(),
    alt_tool: state.config.annotate_alt_tool.clone(),
    default_tool: state.config.annotate_default_tool.clone(),
    right_default_tool: state.config.annotate_right_default_tool.clone(),
    right_shift_tool: state.config.annotate_right_shift_tool.clone(),
    right_ctrl_tool: state.config.annotate_right_ctrl_tool.clone(),
    right_alt_tool: state.config.annotate_right_alt_tool.clone(),
  }
}

/// Annotation editor: save the composited annotated image.
/// Receives the final image as a base64-encoded PNG from the canvas.
/// If annotation_source_path is set (file annotation), saves next to the original.
#[tauri::command]
pub async fn save_annotated_capture(
  app: AppHandle,
  image_base64: String,
) -> Result<CaptureResultDto, AppError> {
  // Limit decoded size to prevent OOM from crafted input (~100MB decoded)
  const MAX_BASE64_LEN: usize = 134_000_000;
  if image_base64.len() > MAX_BASE64_LEN {
    return Err(AppError::Annotation("Image data too large".to_string()));
  }

  // Decode base64 PNG to RgbaImage
  let png_bytes = base64::Engine::decode(
    &base64::engine::general_purpose::STANDARD,
    &image_base64,
  )
  .map_err(|e| AppError::Annotation(format!("Failed to decode base64: {e}")))?;

  let img = image::load_from_memory_with_format(&png_bytes, image::ImageFormat::Png)
    .map_err(|e| AppError::Annotation(format!("Failed to decode PNG: {e}")))?
    .to_rgba8();

  // Check if this is a file annotation (save beside original) or a capture annotation
  let source_path = app.state::<Mutex<AppState>>().lock_or_recover()
    .annotation_source_path.clone();

  let result = if let Some(ref src) = source_path {
    save_annotated_beside(src, &img, &app)?
  } else {
    finalize_capture(&app, &img)?
  };

  // Close annotation window and clean up state (including is_annotating)
  let app2 = app.clone();
  tauri::async_runtime::spawn(async move { overlay::close_annotation_window(&app2); });

  Ok(result)
}

/// Save annotated image next to the original file as "name-annotated.ext".
fn save_annotated_beside(
  source: &PathBuf,
  img: &RgbaImage,
  app: &AppHandle,
) -> Result<CaptureResultDto, AppError> {
  let stem = source.file_stem()
    .map(|s| s.to_string_lossy().to_string())
    .unwrap_or_else(|| "annotated".to_string());
  let ext = source.extension()
    .map(|e| e.to_string_lossy().to_string())
    .unwrap_or_else(|| "png".to_string());
  let parent = source.parent()
    .ok_or_else(|| AppError::Annotation("Cannot determine parent directory".to_string()))?;

  let out_name = format!("{stem}-annotated.{ext}");
  let out_path = parent.join(&out_name);

  // Encode based on extension
  let dyn_img = image::DynamicImage::ImageRgba8(img.clone());
  dyn_img.save(&out_path)
    .map_err(|e| AppError::Annotation(format!("Failed to save annotated image: {e}")))?;

  let filepath_str = out_path.to_string_lossy().to_string();

  // Update history and tray
  {
    let s = app.state::<Mutex<AppState>>();
    let mut state = s.lock_or_recover();
    state.add_to_history(out_path.clone());
    state.last_saved_path = Some(out_path);
  }
  tray::refresh_tray_menu(app);
  notify_capture(app, Some(&filepath_str));

  // Also copy to clipboard if configured
  let config = app.state::<Mutex<AppState>>().lock_or_recover().config.clone();
  let copied = if config.clipboard_action == "image" {
    capture::copy_to_clipboard(img).unwrap_or(());
    true
  } else {
    false
  };

  Ok(CaptureResultDto { filepath: Some(filepath_str), copied_to_clipboard: copied })
}

/// Open the annotation editor for an existing image file on disk.
pub fn annotate_file_from_path(app: &AppHandle, path: &std::path::Path) -> Result<(), AppError> {
  // Block if already capturing/annotating
  {
    let s = app.state::<Mutex<AppState>>();
    let state = s.lock_or_recover();
    if state.is_capturing || state.is_annotating || state.is_recording {
      return Ok(());
    }
  }

  let img = image::open(path)
    .map_err(|e| AppError::Annotation(format!("Failed to open image: {e}")))?
    .to_rgba8();

  // Write JPEG to temp file for fast loading via asset protocol
  let temp_path = std::env::temp_dir().join("qs_annotation.jpg");
  {
    let rgb = capture::rgba_to_rgb_pub(&img)?;
    let file = std::fs::File::create(&temp_path)
      .map_err(|e| AppError::Annotation(format!("Failed to create temp file: {e}")))?;
    let mut writer = std::io::BufWriter::new(file);
    let mut encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut writer, 90);
    encoder
      .encode(&rgb, rgb.width(), rgb.height(), image::ExtendedColorType::Rgb8)
      .map_err(|e| AppError::Annotation(format!("Failed to encode temp JPEG: {e}")))?;
  }

  {
    let s = app.state::<Mutex<AppState>>();
    let mut state = s.lock_or_recover();
    state.pending_annotation = Some(img);
    state.pending_annotation_path = Some(temp_path);
    state.annotation_source_path = Some(path.to_path_buf());
    state.is_annotating = true;
  }

  if let Err(e) = overlay::open_annotation_window(app) {
    let s = app.state::<Mutex<AppState>>();
    let mut state = s.lock_or_recover();
    state.is_annotating = false;
    state.pending_annotation = None;
    state.pending_annotation_path = None;
    state.annotation_source_path = None;
    return Err(e);
  }

  Ok(())
}

/// Tauri command wrapper for annotate_file_from_path (used by tray file picker).
#[tauri::command]
pub async fn annotate_file(app: AppHandle, path: String) -> Result<(), AppError> {
  annotate_file_from_path(&app, std::path::Path::new(&path))
}

/// Annotation editor: discard without saving.
#[tauri::command]
pub async fn cancel_annotation(app: AppHandle) {
  if let Some(win) = app.get_webview_window("annotation") {
    win.hide().ok();
  }
  let app2 = app.clone();
  tauri::async_runtime::spawn(async move { overlay::close_annotation_window(&app2); });
}

#[tauri::command]
pub async fn cancel_capture(app: AppHandle) -> Result<(), AppError> {
  if let Some(overlay) = app.get_webview_window("overlay") {
    overlay.hide().ok();
  }
  let app2 = app.clone();
  tauri::async_runtime::spawn(async move { overlay::close_overlay(&app2); });
  Ok(())
}

// -- Config commands --

#[tauri::command]
pub fn get_config(app: AppHandle) -> crate::config::AppConfig {
  app.state::<Mutex<AppState>>().lock_or_recover().config.clone()
}

#[tauri::command]
pub async fn save_config(
  app: AppHandle,
  new_config: crate::config::AppConfig,
) -> Result<(), AppError> {
  // Validate format
  match new_config.format.as_str() {
    "png" | "jpg" | "jpeg" | "webp" => {}
    other => return Err(AppError::Config(format!("Invalid image format: {other}"))),
  }

  // Validate capture delay
  match new_config.capture_delay {
    0 | 3 | 5 | 10 => {}
    other => return Err(AppError::Config(format!("Invalid capture delay: {other}"))),
  }

  // Validate capture_mode
  match new_config.capture_mode.as_str() {
    "live" | "freeze" | "instant" => {}
    other => return Err(AppError::Config(format!("Invalid capture mode: {other}"))),
  }

  // Validate recording format
  match new_config.recording_format.as_str() {
    "mp4" | "gif" => {}
    other => return Err(AppError::Config(format!("Invalid recording format: {other}"))),
  }

  // Validate recording FPS
  match new_config.recording_fps {
    10 | 15 | 24 | 30 => {}
    other => return Err(AppError::Config(format!("Invalid recording FPS: {other}"))),
  }

  // Validate GIF settings
  if new_config.gif_max_width < 100 || new_config.gif_max_width > 3840 {
    return Err(AppError::Config(format!(
      "GIF max width must be 100-3840, got {}", new_config.gif_max_width
    )));
  }
  if new_config.gif_max_duration > 120 {
    return Err(AppError::Config(format!(
      "GIF max duration must be 0-120s, got {}", new_config.gif_max_duration
    )));
  }

  // Validate filename_suffix as a chrono format string
  let has_bad_format = chrono::format::strftime::StrftimeItems::new(&new_config.filename_suffix)
    .any(|item| matches!(item, chrono::format::Item::Error));
  if has_bad_format {
    return Err(AppError::Config("Invalid date format in filename suffix".to_string()));
  }

  // Compare with current config to avoid unnecessary side effects during
  // real-time auto-save (hotkey re-registration, startup toggle, etc.).
  let old_config = app.state::<Mutex<AppState>>().lock_or_recover().config.clone();

  let hotkeys_changed = old_config.hotkey_region != new_config.hotkey_region
    || old_config.hotkey_fullscreen != new_config.hotkey_fullscreen
    || old_config.hotkey_window != new_config.hotkey_window
    || old_config.hotkey_ocr != new_config.hotkey_ocr
    || old_config.hotkey_record != new_config.hotkey_record;

  // Only reload hotkeys when they actually changed -- avoids briefly
  // unregistering all global hotkeys on every unrelated settings tweak.
  if hotkeys_changed {
    hotkeys::reload_hotkeys_with_config(&app, &new_config)
      .map_err(|e| AppError::Config(e.to_string()))?;
  }

  // All validation passed -- now create directory (side-effect only on success path)
  if old_config.save_folder != new_config.save_folder {
    let folder = std::path::PathBuf::from(&new_config.save_folder);
    std::fs::create_dir_all(&folder)?;
  }

  app.state::<Mutex<AppState>>().lock_or_recover().config = new_config.clone();
  config::save_config(&app, &new_config)?;

  if old_config.launch_on_startup != new_config.launch_on_startup {
    crate::startup::set_launch_on_startup(&app, new_config.launch_on_startup);
  }
  if old_config.explorer_context_menu != new_config.explorer_context_menu {
    crate::context_menu::set_context_menu(new_config.explorer_context_menu);
  }
  tray::refresh_tray_menu(&app);

  Ok(())
}

#[tauri::command]
pub fn get_default_config() -> crate::config::AppConfig {
  crate::config::AppConfig::default()
}

#[tauri::command]
pub async fn pick_folder(app: AppHandle) -> Result<Option<String>, AppError> {
  use tauri_plugin_dialog::DialogExt;
  let folder = app.dialog().file().blocking_pick_folder();
  Ok(folder.map(|p| p.to_string()))
}

#[tauri::command]
pub fn validate_save_folder(folder: String) -> String {
  let path = std::path::PathBuf::from(&folder);
  if path.exists() {
    if path.is_dir() {
      // Try to check writability by creating a temp file
      let test_path = path.join(".quickshotter_write_test");
      match std::fs::write(&test_path, b"") {
        Ok(_) => {
          std::fs::remove_file(&test_path).ok();
          "ok".to_string()
        }
        Err(_) => "Folder exists but is not writable".to_string(),
      }
    } else {
      "Path exists but is not a directory".to_string()
    }
  } else {
    // Check if parent is writable
    if let Some(parent) = path.parent() {
      if parent.exists() && parent.is_dir() {
        "ok".to_string() // Parent exists, folder can be created
      } else {
        "Parent directory does not exist".to_string()
      }
    } else {
      "Invalid path".to_string()
    }
  }
}

// -- Utility --

/// Crop with bounds clamping to prevent panics from out-of-range coordinates.
fn safe_crop(img: &RgbaImage, x: u32, y: u32, w: u32, h: u32) -> Result<RgbaImage, AppError> {
  let iw = img.width();
  let ih = img.height();
  let x = x.min(iw.saturating_sub(1));
  let y = y.min(ih.saturating_sub(1));
  let w = w.min(iw.saturating_sub(x));
  let h = h.min(ih.saturating_sub(y));
  if w == 0 || h == 0 {
    return Err(AppError::Capture("Crop region outside image bounds".to_string()));
  }
  Ok(image::imageops::crop_imm(img, x, y, w, h).to_image())
}

/// Pre-create the settings window hidden at startup so it opens instantly.
pub fn precreate_settings_window(app: &AppHandle) {
  WebviewWindowBuilder::new(app, "settings", WebviewUrl::App("index.html".into()))
    .title("QuickShotter Settings")
    .inner_size(480.0, 620.0)
    .resizable(false)
    .center()
    .always_on_top(true)
    .visible(false)
    .build()
    .ok();
}

#[tauri::command]
pub fn show_settings(app: AppHandle) {
  show_settings_window(&app);
}

pub fn show_settings_window(app: &AppHandle) {
  if let Some(window) = app.get_webview_window("settings") {
    window.show().ok();
    window.set_focus().ok();
  } else {
    // Fallback: create if it doesn't exist (shouldn't happen)
    precreate_settings_window(app);
    if let Some(window) = app.get_webview_window("settings") {
      window.show().ok();
      window.set_focus().ok();
    }
  }
}

fn notify_capture(app: &AppHandle, filepath: Option<&str>) {
  use tauri_plugin_notification::NotificationExt;

  let (title, body) = match filepath {
    Some(fp) => {
      let filename = std::path::Path::new(fp)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "screenshot".to_string());
      ("Screenshot saved".to_string(), filename)
    }
    None => ("Screenshot captured".to_string(), "Copied to clipboard".to_string()),
  };

  let mut builder = app.notification()
    .builder()
    .title(&title)
    .body(&body);
  if let Some(fp) = filepath {
    builder = builder.extra("filepath", fp);
  }
  builder.show().ok();
}

// -- Imgur upload --

/// Upload the most recent capture (from history) to catbox.moe.
#[tauri::command]
pub async fn upload_last_to_imgur(app: AppHandle) -> Result<String, AppError> {
  let path = {
    let s = app.state::<Mutex<AppState>>();
    let state = s.lock_or_recover();
    state.capture_history.back().cloned()
      .ok_or_else(|| AppError::Upload("No recent capture to upload".to_string()))?
  };

  let img = image::open(&path)
    .map_err(|e| AppError::Upload(format!("Failed to read image: {e}")))?
    .to_rgba8();

  let url = crate::catbox::upload(&img).await?;
  capture::copy_text_to_clipboard(&url)?;

  use tauri_plugin_notification::NotificationExt;
  app.notification()
    .builder()
    .title("Uploaded to catbox.moe")
    .body(&url)
    .show()
    .ok();

  // Open in default browser
  #[cfg(target_os = "windows")]
  { std::process::Command::new("cmd").args(["/c", "start", &url]).spawn().ok(); }
  #[cfg(target_os = "macos")]
  { std::process::Command::new("open").arg(&url).spawn().ok(); }

  Ok(url)
}

// -- Recording helpers --

fn open_recording_indicator(app: &AppHandle, pos: Option<(f64, f64)>) {
  if app.get_webview_window("recording-indicator").is_some() {
    return;
  }
  let mut builder = WebviewWindowBuilder::new(app, "recording-indicator", WebviewUrl::App("recording-indicator.html".into()))
    .title("Recording")
    .transparent(true)
    .decorations(false)
    .shadow(false)
    .always_on_top(true)
    .resizable(false)
    .inner_size(340.0, 36.0)
    .skip_taskbar(true);
  if let Some((x, y)) = pos {
    // Position above the recording region's top-left corner
    builder = builder.position(x, (y - 42.0).max(0.0));
  }
  builder.build().ok();
}

fn close_recording_indicator(app: &AppHandle) {
  if let Some(win) = app.get_webview_window("recording-indicator") {
    win.destroy().ok();
  }
}

fn build_pipeline_config(
  config: &crate::config::AppConfig,
  region: Option<crate::recording::RecordingRegion>,
  capture_source: crate::recording::CaptureSource,
) -> Result<crate::recording::PipelineConfig, AppError> {
  let format = match config.recording_format.as_str() {
    "gif" => crate::recording::RecordingFormat::Gif,
    _ => crate::recording::RecordingFormat::Mp4,
  };

  let ext = match format {
    crate::recording::RecordingFormat::Gif => "gif",
    crate::recording::RecordingFormat::Mp4 => "mp4",
  };

  let folder = std::path::PathBuf::from(&config.save_folder);
  std::fs::create_dir_all(&folder)?;

  let timestamp = chrono::Local::now().format("%Y-%m-%d_%H-%M-%S").to_string();
  let filename = format!("recording_{}.{}", timestamp, ext);
  let output_path = folder.join(filename);

  Ok(crate::recording::PipelineConfig {
    format,
    fps: config.recording_fps,
    region,
    output_path,
    gif_max_duration_secs: config.gif_max_duration,
    gif_max_width: config.gif_max_width,
    capture_source,
  })
}

fn notify_recording(app: &AppHandle, path: &std::path::Path) {
  use tauri_plugin_notification::NotificationExt;
  let filename = path
    .file_name()
    .map(|n| n.to_string_lossy().to_string())
    .unwrap_or_else(|| "recording".to_string());
  app.notification()
    .builder()
    .title("Recording saved")
    .body(&filename)
    .extra("filepath", path.to_string_lossy().to_string())
    .show()
    .ok();
}

/// Reveal a file in the system file explorer.
#[tauri::command]
pub fn reveal_file(path: String) {
  let p = std::path::PathBuf::from(&path);
  if p.exists() {
    tray::open_in_explorer(&p);
  }
}

// -- Recording commands --

#[derive(serde::Serialize)]
pub struct RecordingResultDto {
  pub filepath: Option<String>,
  pub duration_secs: f64,
  pub format: String,
}

#[derive(serde::Serialize)]
pub struct RecordingStateDto {
  pub is_recording: bool,
  pub elapsed_secs: f64,
  pub format: String,
  pub saved_filepath: Option<String>,
}

/// Toggle fullscreen recording on/off.
/// If not recording: start recording the entire screen.
/// If already recording: stop and save.
#[tauri::command]
pub async fn toggle_recording(app: AppHandle) -> Result<RecordingResultDto, AppError> {
  let is_recording = app.state::<Mutex<AppState>>().lock_or_recover().is_recording;

  if is_recording {
    return stop_recording(app).await;
  }

  // Guard: can't record while capturing or annotating
  let config = {
    let s = app.state::<Mutex<AppState>>();
    let state = s.lock_or_recover();
    if state.is_capturing || state.is_annotating {
      return Ok(RecordingResultDto { filepath: None, duration_secs: 0.0, format: String::new() });
    }
    state.config.clone()
  };

  let pipeline_config = build_pipeline_config(&config, None, crate::recording::CaptureSource::SingleMonitor(0))?;
  let handle = crate::recording::pipeline::start_pipeline(pipeline_config)?;

  {
    let s = app.state::<Mutex<AppState>>();
    let mut state = s.lock_or_recover();
    state.is_recording = true;
    state.recording_region = None;
    state.recording_start = Some(std::time::Instant::now());
    state.recording_stop_signal = Some(handle.stop_signal.clone());
    state.pipeline_handle = Some(handle);
  }

  tray::refresh_tray_menu(&app);
  open_recording_indicator(&app, None);

  Ok(RecordingResultDto { filepath: None, duration_secs: 0.0, format: config.recording_format })
}

/// Start recording a specific region (called from overlay while it's still open).
#[tauri::command]
pub async fn start_region_recording(
  app: AppHandle,
  x: u32,
  y: u32,
  width: u32,
  height: u32,
  indicator_x: Option<f64>,
  indicator_y: Option<f64>,
) -> Result<(), AppError> {
  let config = {
    let s = app.state::<Mutex<AppState>>();
    let mut state = s.lock_or_recover();
    if state.is_annotating || state.is_recording {
      return Ok(());
    }
    // Set recording flag early to prevent duplicate starts from rapid calls.
    // Cleared on error below.
    state.is_recording = true;
    // Clear capturing flag: overlay stays open but transitions to recording boundary mode
    state.is_capturing = false;
    state.config.clone()
  };

  // The overlay spans the full virtual desktop, so (x, y) are physical pixel
  // offsets from the overlay's top-left corner (= desktop bounds origin).
  // Convert to absolute desktop coords, find which monitor the selection fits
  // in, or use multi-monitor capture if it crosses boundaries.
  let bounds = capture::get_desktop_bounds()?;
  let monitors = xcap::Monitor::all().map_err(|e| AppError::Recording(format!("Failed to enumerate monitors: {e}")))?;
  if monitors.is_empty() {
    return Err(AppError::Recording("No monitors found".to_string()));
  }

  // Absolute desktop position of selection
  let abs_x = x as i32 + bounds.x;
  let abs_y = y as i32 + bounds.y;
  let abs_r = abs_x + width as i32;
  let abs_b = abs_y + height as i32;

  // Check if the selection fits entirely within any single monitor
  let mut single_monitor: Option<usize> = None;
  for (i, m) in monitors.iter().enumerate() {
    let mx = m.x().unwrap_or(0);
    let my = m.y().unwrap_or(0);
    let mw = m.width().unwrap_or(0) as i32;
    let mh = m.height().unwrap_or(0) as i32;
    if abs_x >= mx && abs_y >= my && abs_r <= mx + mw && abs_b <= my + mh {
      single_monitor = Some(i);
      break;
    }
  }

  let (region, capture_source) = if let Some(idx) = single_monitor {
    // Selection fits in one monitor -- use fast single-monitor capture
    let mon = &monitors[idx];
    let mon_x = mon.x().unwrap_or(0);
    let mon_y = mon.y().unwrap_or(0);
    let adj_x = (abs_x - mon_x).max(0) as u32;
    let adj_y = (abs_y - mon_y).max(0) as u32;
    eprintln!("recording: single-monitor[{}] overlay=({},{} {}x{}) adjusted=({},{})",
      idx, x, y, width, height, adj_x, adj_y);
    (
      crate::recording::RecordingRegion { x: adj_x, y: adj_y, width, height },
      crate::recording::CaptureSource::SingleMonitor(idx),
    )
  } else {
    // Selection crosses monitors -- use multi-monitor capture.
    // Region coords stay overlay-relative (= desktop-origin-relative).
    eprintln!("recording: cross-monitor overlay=({},{} {}x{}) abs=({},{} -> {},{})",
      x, y, width, height, abs_x, abs_y, abs_r, abs_b);
    (
      crate::recording::RecordingRegion { x, y, width, height },
      crate::recording::CaptureSource::FullDesktop,
    )
  };

  let pipeline_config = match build_pipeline_config(&config, Some(region.clone()), capture_source) {
    Ok(c) => c,
    Err(e) => {
      app.state::<Mutex<AppState>>().lock_or_recover().is_recording = false;
      return Err(e);
    }
  };
  let handle = match crate::recording::pipeline::start_pipeline(pipeline_config) {
    Ok(h) => h,
    Err(e) => {
      app.state::<Mutex<AppState>>().lock_or_recover().is_recording = false;
      return Err(e);
    }
  };

  {
    let s = app.state::<Mutex<AppState>>();
    let mut state = s.lock_or_recover();
    state.recording_region = Some(region);
    state.recording_start = Some(std::time::Instant::now());
    state.recording_stop_signal = Some(handle.stop_signal.clone());
    state.pipeline_handle = Some(handle);
  }

  tray::refresh_tray_menu(&app);

  // Position indicator at the top-left of the selection region.
  // indicator_x/Y are CSS pixel offsets within the overlay, which starts
  // at the desktop bounds origin.
  let indicator_pos = match (indicator_x, indicator_y) {
    (Some(ix), Some(iy)) => Some((bounds.x as f64 + ix, bounds.y as f64 + iy)),
    _ => None,
  };
  open_recording_indicator(&app, indicator_pos);

  Ok(())
}

/// Stop the current recording and save the file.
#[tauri::command]
pub async fn stop_recording(app: AppHandle) -> Result<RecordingResultDto, AppError> {
  let (was_recording, elapsed, format, pipeline, copy_to_clip) = {
    let s = app.state::<Mutex<AppState>>();
    let mut state = s.lock_or_recover();
    let was = state.is_recording;
    let elapsed = state.recording_start
      .map(|t| t.elapsed().as_secs_f64())
      .unwrap_or(0.0);
    let format = state.config.recording_format.clone();
    let pipeline = state.pipeline_handle.take();
    let copy_clip = state.config.clipboard_action == "image";

    state.is_recording = false;
    state.recording_stop_signal = None;
    state.recording_region = None;
    state.recording_start = None;
    (was, elapsed, format, pipeline, copy_clip)
  };

  if !was_recording {
    return Err(AppError::Recording("Not currently recording".to_string()));
  }

  // Close recording boundary overlay if it's still open
  if let Some(overlay) = app.get_webview_window("overlay") {
    overlay.destroy().ok();
  }
  tray::refresh_tray_menu(&app);

  // Wait for pipeline to finish on a blocking thread (it joins two threads).
  // The indicator window auto-closes via polling (sees is_recording=false).
  let output_path = if let Some(handle) = pipeline {
    let result = tauri::async_runtime::spawn_blocking(move || {
      handle.stop_and_wait()
    }).await.map_err(|e| AppError::Recording(format!("Pipeline join failed: {e}")))?;
    match result {
      Ok(path) => {
        let exists = path.exists();
        let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        eprintln!("recording: stop_and_wait returned path={:?} exists={} size={}", path, exists, size);
        // Add to capture history
        let s = app.state::<Mutex<AppState>>();
        let mut state = s.lock_or_recover();
        state.add_to_history(path.clone());
        state.last_saved_path = Some(path.clone());
        drop(state);
        tray::refresh_tray_menu(&app);
        // Copy recording file to clipboard (like Ctrl+C on a file)
        if copy_to_clip {
          if let Err(e) = capture::copy_file_to_clipboard(&path) {
            eprintln!("Failed to copy recording to clipboard: {e}");
          }
        }
        notify_recording(&app, &path);
        Some(path.to_string_lossy().to_string())
      }
      Err(e) => {
        eprintln!("recording: pipeline error: {e}");
        // Still clean up windows on error
        close_recording_indicator(&app);
        return Err(e);
      }
    }
  } else {
    None
  };

  // The indicator manages its own lifecycle (shows "Saved" for ~4s then destroys).
  // Safety net: if JS fails to close it, Rust cleans up after 6s.
  let app2 = app.clone();
  tauri::async_runtime::spawn(async move {
    let _ = tauri::async_runtime::spawn_blocking(|| {
      std::thread::sleep(std::time::Duration::from_secs(6));
    }).await;
    close_recording_indicator(&app2);
  });

  Ok(RecordingResultDto {
    filepath: output_path,
    duration_secs: elapsed,
    format,
  })
}

/// Get current recording state (for the indicator window).
#[tauri::command]
pub fn get_recording_state(app: AppHandle) -> RecordingStateDto {
  let s = app.state::<Mutex<AppState>>();
  let state = s.lock_or_recover();
  let saved = if !state.is_recording {
    state.last_saved_path.as_ref().map(|p| p.to_string_lossy().to_string())
  } else {
    None
  };
  RecordingStateDto {
    is_recording: state.is_recording,
    elapsed_secs: state.recording_start
      .map(|t| t.elapsed().as_secs_f64())
      .unwrap_or(0.0),
    format: state.config.recording_format.clone(),
    saved_filepath: saved,
  }
}
