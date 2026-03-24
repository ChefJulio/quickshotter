use std::sync::Mutex;
use tauri::{AppHandle, Manager};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut};

use crate::config::AppConfig;
use crate::state::{AppState, LockRecover};

pub fn register_hotkeys(app: &AppHandle) -> Result<(), Box<dyn std::error::Error>> {
  let config = {
    let s = app.state::<Mutex<AppState>>();
    let state = s.lock_or_recover();
    state.config.clone()
  };
  register_hotkeys_from_config(app, &config)
}

/// Register hotkeys from a provided config (does NOT read state).
fn register_hotkeys_from_config(
  app: &AppHandle,
  config: &AppConfig,
) -> Result<(), Box<dyn std::error::Error>> {
  // Parse all hotkeys upfront before registering any -- if any fail to parse,
  // we bail out before touching the global shortcut state.
  let region_shortcut: Shortcut = config.hotkey_region.parse().map_err(|e| {
    format!("Invalid region hotkey '{}': {}", config.hotkey_region, e)
  })?;
  let fullscreen_shortcut: Shortcut = config.hotkey_fullscreen.parse().map_err(|e| {
    format!("Invalid fullscreen hotkey '{}': {}", config.hotkey_fullscreen, e)
  })?;
  let window_shortcut: Shortcut = config.hotkey_window.parse().map_err(|e| {
    format!("Invalid window hotkey '{}': {}", config.hotkey_window, e)
  })?;
  let record_shortcut: Shortcut = config.hotkey_record.parse().map_err(|e| {
    format!("Invalid record hotkey '{}': {}", config.hotkey_record, e)
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
      let mode = app.state::<std::sync::Mutex<crate::state::AppState>>()
        .lock_or_recover().config.fullscreen_mode.clone();
      if mode == "select" {
        if let Err(e) = crate::overlay::open_overlay_with_mode(&app, "select_screen") {
          eprintln!("Select screen overlay failed: {e}");
        }
      } else {
        let app = app.clone();
        tauri::async_runtime::spawn(async move {
          if let Err(e) = crate::commands::do_fullscreen_capture(&app).await {
            eprintln!("Fullscreen capture failed: {e}");
          }
        });
      }
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

  app.global_shortcut().on_shortcut(record_shortcut, {
    let app = app.clone();
    move |_app_handle, _shortcut, _event| {
      // If already recording, stop. Otherwise, open region selection overlay.
      let is_recording = app.state::<Mutex<AppState>>().lock_or_recover().is_recording;
      if is_recording {
        let app = app.clone();
        tauri::async_runtime::spawn(async move {
          if let Err(e) = crate::commands::stop_recording(app).await {
            eprintln!("Stop recording failed: {e}");
          }
        });
      } else {
        if let Err(e) = crate::overlay::open_overlay_with_mode(&app, "record_region") {
          eprintln!("Record region overlay failed: {e}");
        }
      }
    }
  })?;

  Ok(())
}

pub fn unregister_all(app: &AppHandle) {
  app.global_shortcut().unregister_all().ok();
}

/// Validate and register hotkeys from a new config.
/// On failure, rolls back to the old config's hotkeys so the user is never
/// left without working shortcuts.
pub fn reload_hotkeys_with_config(
  app: &AppHandle,
  new_config: &AppConfig,
) -> Result<(), Box<dyn std::error::Error>> {
  let old_config = app.state::<Mutex<AppState>>().lock_or_recover().config.clone();

  unregister_all(app);

  if let Err(e) = register_hotkeys_from_config(app, new_config) {
    // Rollback: re-register the old (known-good) hotkeys
    unregister_all(app);
    if let Err(rollback_err) = register_hotkeys_from_config(app, &old_config) {
      eprintln!("Failed to rollback hotkeys: {rollback_err}");
    }
    return Err(e);
  }

  Ok(())
}
