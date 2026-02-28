/// Launch on startup -- cross-platform via tauri-plugin-autostart.
///
/// Handles Windows registry, macOS LaunchAgents, and Linux XDG autostart.

use tauri::AppHandle;
use tauri_plugin_autostart::ManagerExt;

pub fn set_launch_on_startup(app: &AppHandle, enabled: bool) {
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
