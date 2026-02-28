use std::sync::Mutex;
use tauri::{AppHandle, Manager};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut};

use crate::state::AppState;

pub fn register_hotkeys(app: &AppHandle) -> Result<(), Box<dyn std::error::Error>> {
  let (region_str, fullscreen_str, window_str) = {
    let state = app.state::<Mutex<AppState>>();
    let state = state.lock().unwrap();
    (
      state.config.hotkey_region.clone(),
      state.config.hotkey_fullscreen.clone(),
      state.config.hotkey_window.clone(),
    )
  };

  let region_shortcut: Shortcut = region_str.parse().map_err(|e| {
    format!("Invalid region hotkey '{}': {}", region_str, e)
  })?;
  let fullscreen_shortcut: Shortcut = fullscreen_str.parse().map_err(|e| {
    format!("Invalid fullscreen hotkey '{}': {}", fullscreen_str, e)
  })?;
  let window_shortcut: Shortcut = window_str.parse().map_err(|e| {
    format!("Invalid window hotkey '{}': {}", window_str, e)
  })?;

  app.global_shortcut().on_shortcut(region_shortcut, {
    let app = app.clone();
    move |_app_handle, _shortcut, _event| {
      if let Err(e) = crate::overlay::open_overlay(&app) {
        eprintln!("Region capture failed: {e}");
      }
    }
  })?;

  app.global_shortcut().on_shortcut(fullscreen_shortcut, {
    let app = app.clone();
    move |_app_handle, _shortcut, _event| {
      let app = app.clone();
      tauri::async_runtime::spawn(async move {
        if let Err(e) = crate::commands::do_fullscreen_capture(&app).await {
          eprintln!("Fullscreen capture failed: {e}");
        }
      });
    }
  })?;

  app.global_shortcut().on_shortcut(window_shortcut, {
    let app = app.clone();
    move |_app_handle, _shortcut, _event| {
      if let Err(e) = crate::overlay::open_overlay_with_mode(&app, "window") {
        eprintln!("Window capture failed: {e}");
      }
    }
  })?;

  Ok(())
}

pub fn unregister_all(app: &AppHandle) {
  app.global_shortcut().unregister_all().ok();
}

pub fn reload_hotkeys(app: &AppHandle) -> Result<(), Box<dyn std::error::Error>> {
  unregister_all(app);
  register_hotkeys(app)
}
