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

/// Returns "instant" or "freeze" so the overlay JS knows which mode to use.
#[tauri::command]
pub fn get_overlay_mode(app: AppHandle) -> String {
  let state = app.state::<Mutex<AppState>>();
  let state = state.lock().unwrap();
  state.config.capture_mode.clone()
}

/// In freeze mode, the overlay pulls the pre-captured screenshot.
#[tauri::command]
pub fn get_pending_screenshot(app: AppHandle) -> Result<String, AppError> {
  let state = app.state::<Mutex<AppState>>();
  let state = state.lock().unwrap();
  state
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

pub async fn do_fullscreen_capture(app: &AppHandle) -> Result<CaptureResultDto, AppError> {
  let screen = capture::capture_all_monitors()?;
  capture::copy_to_clipboard(&screen.image)?;

  let config = {
    let state = app.state::<Mutex<AppState>>();
    let state = state.lock().unwrap();
    state.config.clone()
  };

  let saved = capture::save_to_disk(&screen.image, &config)?;
  let filepath_str = saved.as_ref().map(|p: &PathBuf| p.to_string_lossy().to_string());

  if let Some(ref path) = saved {
    let state = app.state::<Mutex<AppState>>();
    let mut state = state.lock().unwrap();
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
    // Defer destroy so the invoke response can be sent first
    let app2 = app.clone();
    tauri::async_runtime::spawn(async move { overlay::close_overlay(&app2); });
    return Err(AppError::Capture("Selection too small".to_string()));
  }

  // Check which mode we're in
  let mode = {
    let state = app.state::<Mutex<AppState>>();
    let state = state.lock().unwrap();
    state.config.capture_mode.clone()
  };

  let image = if mode == "freeze" {
    // Freeze mode: crop from the pre-captured screenshot
    let state = app.state::<Mutex<AppState>>();
    let state = state.lock().unwrap();
    let screenshot = state
      .pending_screenshot
      .as_ref()
      .ok_or_else(|| AppError::Capture("No pending screenshot".to_string()))?;
    image::imageops::crop_imm(screenshot, left, top, w, h).to_image()
  } else {
    // Instant mode: overlay is hidden, brief delay then capture
    std::thread::sleep(std::time::Duration::from_millis(50));
    let screen = capture::capture_all_monitors()?;
    image::imageops::crop_imm(&screen.image, left, top, w, h).to_image()
  };

  capture::copy_to_clipboard(&image)?;

  let config = {
    let state = app.state::<Mutex<AppState>>();
    let state = state.lock().unwrap();
    state.config.clone()
  };

  let saved = capture::save_to_disk(&image, &config)?;
  let filepath_str = saved.as_ref().map(|p: &PathBuf| p.to_string_lossy().to_string());

  if let Some(ref path) = saved {
    let state = app.state::<Mutex<AppState>>();
    let mut state = state.lock().unwrap();
    state.add_to_history(path.clone());
    state.last_saved_path = Some(path.clone());
  }

  tray::refresh_tray_menu(&app);
  notify_capture(&app, filepath_str.as_deref());

  // Defer overlay destroy so the invoke response gets sent back first
  let app2 = app.clone();
  tauri::async_runtime::spawn(async move { overlay::close_overlay(&app2); });

  Ok(CaptureResultDto {
    filepath: filepath_str,
    copied_to_clipboard: true,
  })
}

#[tauri::command]
pub async fn cancel_capture(app: AppHandle) -> Result<(), AppError> {
  // Hide immediately, defer destroy so invoke response can be sent
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
  let state = app.state::<Mutex<AppState>>();
  let state = state.lock().unwrap();
  state.config.clone()
}

#[tauri::command]
pub async fn save_config(
  app: AppHandle,
  new_config: crate::config::AppConfig,
) -> Result<(), AppError> {
  let folder = std::path::PathBuf::from(&new_config.save_folder);
  std::fs::create_dir_all(&folder)?;

  {
    let state = app.state::<Mutex<AppState>>();
    let mut state = state.lock().unwrap();
    state.config = new_config.clone();
  }
  config::save_config(&app, &new_config)?;

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
      .inner_size(480.0, 520.0)
      .resizable(false)
      .center()
      .always_on_top(true)
      .build()
      .ok();
  }
}

fn notify_capture(_app: &AppHandle, filepath: Option<&str>) {
  let filepath_owned = filepath.map(|s| s.to_string());

  std::thread::spawn(move || {
    let (title, body) = match filepath_owned.as_deref() {
      Some(fp) => {
        let filename = std::path::Path::new(fp)
          .file_name()
          .map(|n| n.to_string_lossy().to_string())
          .unwrap_or_else(|| "screenshot".to_string());
        ("Screenshot saved".to_string(), filename)
      }
      None => ("Screenshot captured".to_string(), "Copied to clipboard".to_string()),
    };

    #[cfg(target_os = "windows")]
    {
      use tauri_winrt_notification::Toast;

      let fp_clone = filepath_owned.clone();
      let mut toast = Toast::new(Toast::POWERSHELL_APP_ID)
        .title(&title)
        .text1(&body);

      if filepath_owned.is_some() {
        toast = toast
          .add_button("Show in folder", "open_folder")
          .on_activated(move |action| {
            if action.is_none() || action.as_deref() == Some("open_folder") {
              if let Some(ref fp) = fp_clone {
                reveal_in_explorer(std::path::Path::new(fp));
              }
            }
            Ok(())
          });
      }

      toast.show().ok();
    }

    #[cfg(not(target_os = "windows"))]
    {
      // TODO: add notify-rust dependency for macOS/Linux notifications
      eprintln!("{}: {}", title, body);
    }
  });
}

fn reveal_in_explorer(path: &std::path::Path) {
  #[cfg(target_os = "windows")]
  {
    std::process::Command::new("explorer")
      .arg(format!("/select,{}", path.display()))
      .spawn()
      .ok();
  }
  #[cfg(target_os = "macos")]
  {
    std::process::Command::new("open")
      .args(["-R", &path.to_string_lossy()])
      .spawn()
      .ok();
  }
  #[cfg(target_os = "linux")]
  {
    if let Some(parent) = path.parent() {
      std::process::Command::new("xdg-open")
        .arg(parent)
        .spawn()
        .ok();
    }
  }
}
