mod capture;
mod commands;
mod config;
mod error;
mod hotkeys;
mod overlay;
mod startup;
mod state;
mod tray;
mod window_capture;

use std::sync::Mutex;
use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
  tauri::Builder::default()
    .plugin(tauri_plugin_single_instance::init(|_app, _args, _cwd| {}))
    .plugin(tauri_plugin_global_shortcut::Builder::new().build())
    .plugin(tauri_plugin_dialog::init())
    .plugin(tauri_plugin_notification::init())
    .plugin(tauri_plugin_autostart::init(
      tauri_plugin_autostart::MacosLauncher::LaunchAgent,
      None,
    ))
    .invoke_handler(tauri::generate_handler![
      commands::trigger_region_capture,
      commands::trigger_fullscreen_capture,
      commands::complete_region_capture,
      commands::cancel_capture,
      commands::get_overlay_mode,
      commands::get_pending_screenshot,
      commands::get_config,
      commands::save_config,
      commands::pick_folder,
      commands::trigger_window_capture,
      commands::get_window_at_cursor,
      commands::complete_window_capture,
      commands::get_pending_annotation,
      commands::get_annotation_config,
      commands::save_annotated_capture,
      commands::cancel_annotation,
      commands::validate_save_folder,
    ])
    .setup(|app| {
      let config = config::load_config(&app.handle());
      app.manage(Mutex::new(state::AppState::new(config)));

      tray::setup_tray(&app.handle())?;
      if let Err(e) = hotkeys::register_hotkeys(&app.handle()) {
        eprintln!("Failed to register hotkeys: {e}");
        use tauri_plugin_notification::NotificationExt;
        app.handle().notification()
          .builder()
          .title("QuickShotter")
          .body(&format!("Failed to register hotkeys: {}", e))
          .show()
          .ok();
      }

      Ok(())
    })
    .build(tauri::generate_context!())
    .expect("error building QuickShotter")
    .run(|_app, event| {
      // Prevent app from exiting when all windows close (tray app)
      // Only block automatic exit (code None), not intentional app.exit() calls (code Some)
      if let tauri::RunEvent::ExitRequested { code, api, .. } = event {
        if code.is_none() {
          api.prevent_exit();
        }
      }
    });
}
