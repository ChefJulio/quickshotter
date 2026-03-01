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
}

/// Returns "instant", "freeze", or "window" so the overlay JS knows which mode.
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

/// In freeze/window mode, the overlay pulls the pre-captured screenshot.
/// Takes ownership of the base64 data (drops it from state) to free memory
/// since the overlay only needs it once.
#[tauri::command]
pub fn get_pending_screenshot(app: AppHandle) -> Result<String, AppError> {
  app.state::<Mutex<AppState>>().lock_or_recover()
    .pending_base64
    .take()
    .ok_or_else(|| AppError::Capture("No pending screenshot".to_string()))
}

// -- Shared capture finalization --

/// Copy image to clipboard, save to disk, update history, refresh tray, and
/// send a desktop notification.  Called from every capture path to avoid
/// duplicating this pipeline.
fn finalize_capture(app: &AppHandle, img: &RgbaImage) -> Result<CaptureResultDto, AppError> {
  capture::copy_to_clipboard(img)?;

  let config = app.state::<Mutex<AppState>>().lock_or_recover().config.clone();

  let saved = capture::save_to_disk(img, &config)?;
  let filepath_str = saved.as_ref().map(|p: &PathBuf| p.to_string_lossy().to_string());

  if let Some(ref path) = saved {
    let s = app.state::<Mutex<AppState>>();
    let mut state = s.lock_or_recover();
    state.add_to_history(path.clone());
    state.last_saved_path = Some(path.clone());
  }

  tray::refresh_tray_menu(app);
  notify_capture(app, filepath_str.as_deref());

  Ok(CaptureResultDto {
    filepath: filepath_str,
    copied_to_clipboard: true,
  })
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
    if state.is_capturing || state.is_annotating {
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
  let screen = capture::capture_all_monitors()?;

  // Detect likely permission denial (black screenshot) on macOS
  #[cfg(target_os = "macos")]
  if capture::is_likely_blank(&screen.image) {
    use tauri_plugin_notification::NotificationExt;
    app.notification()
      .builder()
      .title("Screen Recording Permission Required")
      .body("Grant permission in System Settings > Privacy & Security > Screen Recording, then restart QuickShotter")
      .show()
      .ok();
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
) -> Result<CaptureResultDto, AppError> {
  // Hide overlay immediately (don't destroy -- we're still inside its webview command)
  if let Some(overlay) = app.get_webview_window("overlay") {
    overlay.hide().ok();
  }

  let result = complete_region_capture_inner(&app, x1, y1, x2, y2).await;

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
) -> Result<CaptureResultDto, AppError> {
  let left = x1.min(x2);
  let top = y1.min(y2);
  let right = x1.max(x2);
  let bottom = y1.max(y2);
  let w = right - left;
  let h = bottom - top;

  if w < 3 || h < 3 {
    return Err(AppError::Capture("Selection too small".to_string()));
  }

  // Check which mode we're in
  let mode = app.state::<Mutex<AppState>>().lock_or_recover().overlay_mode.clone();

  let image = if mode == "freeze" || mode == "window" {
    let s = app.state::<Mutex<AppState>>();
    let state = s.lock_or_recover();
    let screenshot = state
      .pending_screenshot
      .as_ref()
      .ok_or_else(|| AppError::Capture("No pending screenshot".to_string()))?;
    safe_crop(screenshot, left, top, w, h)?
  } else {
    // Wait for the overlay window to fully de-render before capturing.
    // Spawn on a blocking thread to avoid blocking the async worker.
    let _ = tauri::async_runtime::spawn_blocking(|| {
      std::thread::sleep(std::time::Duration::from_millis(150));
    }).await;
    let screen = capture::capture_all_monitors()?;
    safe_crop(&screen.image, left, top, w, h)?
  };

  // Check if we should open annotation editor
  let annotate = app.state::<Mutex<AppState>>().lock_or_recover().config.annotate_captures;

  if annotate {
    return open_annotation_for_image(app, image).await;
  }

  finalize_capture(app, &image)
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
) -> Result<CaptureResultDto, AppError> {
  // Hide overlay immediately
  if let Some(overlay) = app.get_webview_window("overlay") {
    overlay.hide().ok();
  }

  let result = complete_window_capture_inner(&app, left, top, right, bottom).await;

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
) -> Result<CaptureResultDto, AppError> {
  if right <= left || bottom <= top {
    return Err(AppError::Capture("Invalid window bounds".to_string()));
  }

  let w = (right - left) as u32;
  let h = (bottom - top) as u32;

  if w < 3 || h < 3 {
    return Err(AppError::Capture("Window too small".to_string()));
  }

  // Capture fresh after overlay is hidden (outer function already hid it)
  let _ = tauri::async_runtime::spawn_blocking(|| {
    std::thread::sleep(std::time::Duration::from_millis(150));
  }).await;

  let screen = capture::capture_all_monitors()?;
  let crop_x = (left - screen.origin_x).max(0) as u32;
  let crop_y = (top - screen.origin_y).max(0) as u32;
  let image = safe_crop(&screen.image, crop_x, crop_y, w, h)?;

  // Check if we should open annotation editor
  let annotate = app.state::<Mutex<AppState>>().lock_or_recover().config.annotate_captures;

  if annotate {
    return open_annotation_for_image(app, image).await;
  }

  finalize_capture(app, &image)
}

// -- Annotation commands --

/// Store image for annotation and open the editor window.
/// Sets `is_annotating` to block concurrent captures while the editor is open.
async fn open_annotation_for_image(
  app: &AppHandle,
  image: image::RgbaImage,
) -> Result<CaptureResultDto, AppError> {
  let base64 = capture::image_to_base64_png(&image)?;

  {
    let s = app.state::<Mutex<AppState>>();
    let mut state = s.lock_or_recover();
    state.pending_annotation = Some(image);
    state.pending_annotation_base64 = Some(base64);
    state.is_annotating = true;
  }

  if let Err(e) = overlay::open_annotation_window(app) {
    // Reset annotation state so future captures aren't permanently blocked
    let s = app.state::<Mutex<AppState>>();
    let mut state = s.lock_or_recover();
    state.is_annotating = false;
    state.pending_annotation = None;
    state.pending_annotation_base64 = None;
    return Err(e);
  }

  // Return a placeholder -- the actual save happens from the annotation editor
  Ok(CaptureResultDto {
    filepath: None,
    copied_to_clipboard: false,
  })
}

/// Annotation editor: fetch the pending image as base64 PNG.
/// Takes ownership (drops from state) to free memory since it's only needed once.
#[tauri::command]
pub fn get_pending_annotation(app: AppHandle) -> Result<String, AppError> {
  app.state::<Mutex<AppState>>().lock_or_recover()
    .pending_annotation_base64
    .take()
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
  }
}

/// Annotation editor: save the composited annotated image.
/// Receives the final image as a base64-encoded PNG from the canvas.
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

  let result = finalize_capture(&app, &img)?;

  // Close annotation window and clean up state (including is_annotating)
  let app2 = app.clone();
  tauri::async_runtime::spawn(async move { overlay::close_annotation_window(&app2); });

  Ok(result)
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

  // Validate capture_mode
  match new_config.capture_mode.as_str() {
    "instant" | "freeze" => {}
    other => return Err(AppError::Config(format!("Invalid capture mode: {other}"))),
  }

  // Validate filename_suffix as a chrono format string
  let has_bad_format = chrono::format::strftime::StrftimeItems::new(&new_config.filename_suffix)
    .any(|item| matches!(item, chrono::format::Item::Error));
  if has_bad_format {
    return Err(AppError::Config("Invalid date format in filename suffix".to_string()));
  }

  // Validate hotkeys before persisting -- avoids saving config with broken hotkeys
  hotkeys::reload_hotkeys_with_config(&app, &new_config)
    .map_err(|e| AppError::Config(e.to_string()))?;

  // All validation passed -- now create directory (side-effect only on success path)
  let folder = std::path::PathBuf::from(&new_config.save_folder);
  std::fs::create_dir_all(&folder)?;

  app.state::<Mutex<AppState>>().lock_or_recover().config = new_config.clone();
  config::save_config(&app, &new_config)?;

  crate::startup::set_launch_on_startup(&app, new_config.launch_on_startup);
  tray::refresh_tray_menu(&app);

  Ok(())
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

pub fn show_settings_window(app: &AppHandle) {
  if let Some(window) = app.get_webview_window("settings") {
    window.show().ok();
    window.set_focus().ok();
  } else {
    WebviewWindowBuilder::new(app, "settings", WebviewUrl::App("index.html".into()))
      .title("QuickShotter Settings")
      .inner_size(480.0, 800.0)
      .resizable(false)
      .center()
      .always_on_top(true)
      .build()
      .ok();
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

  app.notification()
    .builder()
    .title(&title)
    .body(&body)
    .show()
    .ok();
}
