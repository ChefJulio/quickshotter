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

/// In freeze/window mode, the overlay pulls the pre-captured screenshot.
#[tauri::command]
pub fn get_pending_screenshot(app: AppHandle) -> Result<String, AppError> {
  app.state::<Mutex<AppState>>().lock_or_recover()
    .pending_base64
    .clone()
    .ok_or_else(|| AppError::Capture("No pending screenshot".to_string()))
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
  let screen = capture::capture_all_monitors()?;

  let annotate = app.state::<Mutex<AppState>>().lock_or_recover().config.annotate_captures;

  if annotate {
    return open_annotation_for_image(app, screen.image).await;
  }

  capture::copy_to_clipboard(&screen.image)?;

  let config = app.state::<Mutex<AppState>>().lock_or_recover().config.clone();

  let saved = capture::save_to_disk(&screen.image, &config)?;
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

#[tauri::command]
pub async fn complete_region_capture(
  app: AppHandle,
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

  // Hide overlay immediately (don't destroy -- we're still inside its webview command)
  if let Some(overlay) = app.get_webview_window("overlay") {
    overlay.hide().ok();
  }

  if w < 3 || h < 3 {
    let app2 = app.clone();
    tauri::async_runtime::spawn(async move { overlay::close_overlay(&app2); });
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
    std::thread::sleep(std::time::Duration::from_millis(50));
    let screen = capture::capture_all_monitors()?;
    safe_crop(&screen.image, left, top, w, h)?
  };

  // Defer overlay destroy
  let app2 = app.clone();
  tauri::async_runtime::spawn(async move { overlay::close_overlay(&app2); });

  // Check if we should open annotation editor
  let annotate = app.state::<Mutex<AppState>>().lock_or_recover().config.annotate_captures;

  if annotate {
    return open_annotation_for_image(&app, image).await;
  }

  capture::copy_to_clipboard(&image)?;

  let config = app.state::<Mutex<AppState>>().lock_or_recover().config.clone();

  let saved = capture::save_to_disk(&image, &config)?;
  let filepath_str = saved.as_ref().map(|p: &PathBuf| p.to_string_lossy().to_string());

  if let Some(ref path) = saved {
    let s = app.state::<Mutex<AppState>>();
    let mut state = s.lock_or_recover();
    state.add_to_history(path.clone());
    state.last_saved_path = Some(path.clone());
  }

  tray::refresh_tray_menu(&app);
  notify_capture(&app, filepath_str.as_deref());

  Ok(CaptureResultDto {
    filepath: filepath_str,
    copied_to_clipboard: true,
  })
}

// -- Window capture commands --

#[tauri::command]
pub fn get_window_at_cursor(app: AppHandle) -> Option<window_capture::WindowRect> {
  let exclude_id = app.state::<Mutex<AppState>>().lock_or_recover().overlay_window_id;
  let (cx, cy) = window_capture::get_cursor_pos();
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

  if right <= left || bottom <= top {
    let app2 = app.clone();
    tauri::async_runtime::spawn(async move { overlay::close_overlay(&app2); });
    return Err(AppError::Capture("Invalid window bounds".to_string()));
  }

  let w = (right - left) as u32;
  let h = (bottom - top) as u32;

  if w < 3 || h < 3 {
    let app2 = app.clone();
    tauri::async_runtime::spawn(async move { overlay::close_overlay(&app2); });
    return Err(AppError::Capture("Window too small".to_string()));
  }

  // Get the origin offset from the virtual desktop
  let bounds = capture::get_desktop_bounds()?;
  let origin_x = bounds.x;
  let origin_y = bounds.y;

  let crop_x = (left - origin_x).max(0) as u32;
  let crop_y = (top - origin_y).max(0) as u32;

  let image = {
    let s = app.state::<Mutex<AppState>>();
    let state = s.lock_or_recover();
    let screenshot = state
      .pending_screenshot
      .as_ref()
      .ok_or_else(|| AppError::Capture("No pending screenshot".to_string()))?;
    safe_crop(screenshot, crop_x, crop_y, w, h)?
  };

  // Defer overlay destroy
  let app2 = app.clone();
  tauri::async_runtime::spawn(async move { overlay::close_overlay(&app2); });

  // Check if we should open annotation editor
  let annotate = app.state::<Mutex<AppState>>().lock_or_recover().config.annotate_captures;

  if annotate {
    return open_annotation_for_image(&app, image).await;
  }

  capture::copy_to_clipboard(&image)?;

  let config = app.state::<Mutex<AppState>>().lock_or_recover().config.clone();

  let saved = capture::save_to_disk(&image, &config)?;
  let filepath_str = saved.as_ref().map(|p: &PathBuf| p.to_string_lossy().to_string());

  if let Some(ref path) = saved {
    let s = app.state::<Mutex<AppState>>();
    let mut state = s.lock_or_recover();
    state.add_to_history(path.clone());
    state.last_saved_path = Some(path.clone());
  }

  tray::refresh_tray_menu(&app);
  notify_capture(&app, filepath_str.as_deref());

  Ok(CaptureResultDto {
    filepath: filepath_str,
    copied_to_clipboard: true,
  })
}

// -- Annotation commands --

/// Store image for annotation and open the editor window.
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
  }

  overlay::open_annotation_window(app)?;

  // Return a placeholder -- the actual save happens from the annotation editor
  Ok(CaptureResultDto {
    filepath: None,
    copied_to_clipboard: false,
  })
}

/// Annotation editor: fetch the pending image as base64 PNG.
#[tauri::command]
pub fn get_pending_annotation(app: AppHandle) -> Result<String, AppError> {
  app.state::<Mutex<AppState>>().lock_or_recover()
    .pending_annotation_base64
    .clone()
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

  capture::copy_to_clipboard(&img)?;

  let config = app.state::<Mutex<AppState>>().lock_or_recover().config.clone();

  let saved = capture::save_to_disk(&img, &config)?;
  let filepath_str = saved.as_ref().map(|p: &PathBuf| p.to_string_lossy().to_string());

  if let Some(ref path) = saved {
    let s = app.state::<Mutex<AppState>>();
    let mut state = s.lock_or_recover();
    state.add_to_history(path.clone());
    state.last_saved_path = Some(path.clone());
  }

  tray::refresh_tray_menu(&app);
  notify_capture(&app, filepath_str.as_deref());

  // Close annotation window and clean up state
  let app2 = app.clone();
  tauri::async_runtime::spawn(async move { overlay::close_annotation_window(&app2); });

  Ok(CaptureResultDto {
    filepath: filepath_str,
    copied_to_clipboard: true,
  })
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
  let folder = std::path::PathBuf::from(&new_config.save_folder);
  std::fs::create_dir_all(&folder)?;

  // Validate filename_suffix as a chrono format string
  let has_bad_format = chrono::format::strftime::StrftimeItems::new(&new_config.filename_suffix)
    .any(|item| matches!(item, chrono::format::Item::Error));
  if has_bad_format {
    return Err(AppError::Config("Invalid date format in filename suffix".to_string()));
  }

  app.state::<Mutex<AppState>>().lock_or_recover().config = new_config.clone();
  config::save_config(&app, &new_config)?;

  hotkeys::reload_hotkeys(&app).map_err(|e| AppError::Config(e.to_string()))?;
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
