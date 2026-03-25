/// Launch on startup via tauri-plugin-autostart.
///
/// Handles Windows registry and macOS LaunchAgents.

use tauri::AppHandle;
#[cfg(not(debug_assertions))]
use tauri_plugin_autostart::ManagerExt;

pub fn set_launch_on_startup(app: &AppHandle, enabled: bool) {
  // In debug builds, skip autostart registration -- the debug binary shows a
  // console window and isn't suitable for startup launch.
  #[cfg(debug_assertions)]
  {
    if enabled {
      eprintln!("startup: skipping autostart in debug build (use installed release instead)");
    }
    let _ = app;
    return;
  }

  #[cfg(not(debug_assertions))]
  {
    let manager = app.autolaunch();
    let result = if enabled {
      manager.enable()
    } else {
      manager.disable()
    };
    if let Err(e) = result {
      eprintln!("startup: failed to set autostart: {e}");
    }
  }
}
