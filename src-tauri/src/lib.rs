mod capture;
mod commands;
mod config;
mod error;
mod hotkeys;
mod overlay;
mod state;
mod tray;

use std::sync::Mutex;
use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
  tauri::Builder::default()
    .plugin(tauri_plugin_single_instance::init(|_app, _args, _cwd| {}))
    .plugin(tauri_plugin_global_shortcut::Builder::new().build())
    .plugin(tauri_plugin_dialog::init())
    .invoke_handler(tauri::generate_handler![
      commands::trigger_region_capture,
      commands::trigger_fullscreen_capture,
      commands::complete_region_capture,
      commands::cancel_capture,
      commands::get_config,
      commands::save_config,
      commands::pick_folder,
    ])
    .setup(|app| {
      let config = config::load_config(&app.handle());
      app.manage(Mutex::new(state::AppState::new(config)));

      tray::setup_tray(&app.handle())?;
      if let Err(e) = hotkeys::register_hotkeys(&app.handle()) {
        eprintln!("Failed to register hotkeys: {e}");
      }

      Ok(())
    })
    .run(tauri::generate_context!())
    .expect("error while running QuickShotter");
}
