use std::path::PathBuf;
use std::sync::Mutex;
use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder};

use crate::capture;
use crate::config;
use crate::error::AppError;
use crate::hotkeys;
use crate::overlay;
use crate::state::AppState;
use crate::tray;

#[derive(serde::Serialize)]
pub struct CaptureResultDto {
  pub filepath: Option<String>,
  pub copied_to_clipboard: bool,
}

// -- Capture commands --

#[tauri::command]
pub async fn trigger_region_capture(app: AppHandle) -> Result<(), AppError> {
  overlay::open_overlay(&app).await
}

#[tauri::command]
pub async fn trigger_fullscreen_capture(app: AppHandle) -> Result<CaptureResultDto, AppError> {
  do_fullscreen_capture(&app).await
}

pub async fn do_fullscreen_capture(app: &AppHandle) -> Result<CaptureResultDto, AppError> {
  let screenshot = capture::capture_all_monitors()?;
  capture::copy_to_clipboard(&screenshot)?;

  let config = {
    let state = app.state::<Mutex<AppState>>();
    let state = state.lock().unwrap();
    state.config.clone()
  };

  let saved = capture::save_to_disk(&screenshot, &config)?;
  let filepath_str = saved.as_ref().map(|p: &PathBuf| p.to_string_lossy().to_string());

  if let Some(ref path) = saved {
    let state = app.state::<Mutex<AppState>>();
    let mut state = state.lock().unwrap();
    state.add_to_history(path.clone());
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
  // Get the pending screenshot and crop
  let cropped = {
    let state = app.state::<Mutex<AppState>>();
    let state = state.lock().unwrap();
    let screenshot = state
      .pending_screenshot
      .as_ref()
      .ok_or_else(|| AppError::Capture("No pending screenshot".to_string()))?;

    let left = x1.min(x2);
    let top = y1.min(y2);
    let right = x1.max(x2);
    let bottom = y1.max(y2);
    let w = right - left;
    let h = bottom - top;

    if w < 3 || h < 3 {
      return Err(AppError::Capture("Selection too small".to_string()));
    }

    image::imageops::crop_imm(screenshot, left, top, w, h).to_image()
  };

  // Close overlay first
  overlay::close_overlay(&app);

  // Copy to clipboard and save
  capture::copy_to_clipboard(&cropped)?;

  let config = {
    let state = app.state::<Mutex<AppState>>();
    let state = state.lock().unwrap();
    state.config.clone()
  };

  let saved = capture::save_to_disk(&cropped, &config)?;
  let filepath_str = saved.as_ref().map(|p: &PathBuf| p.to_string_lossy().to_string());

  if let Some(ref path) = saved {
    let state = app.state::<Mutex<AppState>>();
    let mut state = state.lock().unwrap();
    state.add_to_history(path.clone());
  }

  tray::refresh_tray_menu(&app);
  notify_capture(&app, filepath_str.as_deref());

  Ok(CaptureResultDto {
    filepath: filepath_str,
    copied_to_clipboard: true,
  })
}

#[tauri::command]
pub async fn cancel_capture(app: AppHandle) -> Result<(), AppError> {
  overlay::close_overlay(&app);
  Ok(())
}

// -- Config commands --

#[tauri::command]
pub fn get_config(app: AppHandle) -> crate::config::AppConfig {
  let state = app.state::<Mutex<AppState>>();
  let state = state.lock().unwrap();
  state.config.clone()
}

#[tauri::command]
pub async fn save_config(
  app: AppHandle,
  new_config: crate::config::AppConfig,
) -> Result<(), AppError> {
  // Ensure save folder exists
  let folder = std::path::PathBuf::from(&new_config.save_folder);
  std::fs::create_dir_all(&folder)?;

  // Update state and persist
  {
    let state = app.state::<Mutex<AppState>>();
    let mut state = state.lock().unwrap();
    state.config = new_config.clone();
  }
  config::save_config(&app, &new_config)?;

  // Reload hotkeys with new config
  hotkeys::reload_hotkeys(&app).map_err(|e| AppError::Config(e.to_string()))?;

  Ok(())
}

#[tauri::command]
pub async fn pick_folder(app: AppHandle) -> Result<Option<String>, AppError> {
  use tauri_plugin_dialog::DialogExt;
  let folder = app.dialog().file().blocking_pick_folder();
  Ok(folder.map(|p| p.to_string()))
}

// -- Utility --

pub fn show_settings_window(app: &AppHandle) {
  if let Some(window) = app.get_webview_window("settings") {
    window.show().ok();
    window.set_focus().ok();
  } else {
    WebviewWindowBuilder::new(app, "settings", WebviewUrl::App("index.html".into()))
      .title("QuickShotter Settings")
      .inner_size(480.0, 400.0)
      .resizable(false)
      .center()
      .always_on_top(true)
      .build()
      .ok();
  }
}

fn notify_capture(app: &AppHandle, filepath: Option<&str>) {
  if let Some(tray) = app.tray_by_id("main") {
    let msg = match filepath {
      Some(fp) => {
        std::path::Path::new(fp)
          .file_name()
          .map(|n| n.to_string_lossy().to_string())
          .unwrap_or_else(|| "Captured".to_string())
      }
      None => "Copied to clipboard".to_string(),
    };
    tray.set_tooltip(Some(&msg)).ok();
  }
}
