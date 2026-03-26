mod capture;
mod commands;
mod config;
mod coords;
mod context_menu;
mod error;
mod hotkeys;
mod catbox;
mod ocr;
mod overlay;
mod recording;
mod startup;
mod state;
mod tray;
mod window_capture;

use std::sync::Mutex;
use tauri::Manager;

use state::LockRecover;

/// Parse CLI args for --annotate <path> and open the annotation editor.
fn handle_cli_args(app: &tauri::AppHandle, args: &[String]) {
  let mut iter = args.iter();
  while let Some(arg) = iter.next() {
    if arg == "--annotate" {
      if let Some(path) = iter.next() {
        let p = std::path::PathBuf::from(path);
        if p.exists() {
          if let Err(e) = commands::annotate_file_from_path(app, &p) {
            eprintln!("Failed to annotate file: {e}");
          }
        } else {
          eprintln!("File not found: {}", p.display());
        }
      }
      return;
    }
    // Also handle bare file path (no --annotate flag) for drag-and-drop / open-with
    let p = std::path::PathBuf::from(arg);
    if p.extension().map_or(false, |ext| {
      matches!(ext.to_string_lossy().to_lowercase().as_str(), "png" | "jpg" | "jpeg" | "webp" | "bmp")
    }) && p.exists() {
      if let Err(e) = commands::annotate_file_from_path(app, &p) {
        eprintln!("Failed to annotate file: {e}");
      }
      return;
    }
  }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
  tauri::Builder::default()
    .plugin(tauri_plugin_single_instance::init(|app, args, _cwd| {
      // Second instance launched (e.g. Explorer context menu "Annotate with QuickShotter").
      // Parse args and open the annotation editor if --annotate <path> is present.
      handle_cli_args(app, &args);
    }))
    .plugin(tauri_plugin_global_shortcut::Builder::new().build())
    .plugin(tauri_plugin_dialog::init())
    .plugin(tauri_plugin_notification::init())
    .plugin(tauri_plugin_autostart::init(
      tauri_plugin_autostart::MacosLauncher::LaunchAgent,
      None,
    ))
    .plugin(tauri_plugin_updater::Builder::new().build())
    .plugin(tauri_plugin_process::init())
    .invoke_handler(tauri::generate_handler![
      commands::get_capture_delay,
      commands::prepare_delayed_capture,
      commands::execute_delayed_capture,
      commands::cancel_delayed_capture,
      commands::trigger_region_capture,
      commands::trigger_fullscreen_capture,
      commands::complete_region_capture,
      commands::cancel_capture,
      commands::get_overlay_mode,
      commands::get_overlay_origin,
      commands::get_pending_screenshot,
      commands::get_config,
      commands::save_config,
      commands::get_default_config,
      commands::pick_folder,
      commands::trigger_window_capture,
      commands::get_window_at_cursor,
      commands::complete_window_capture,
      commands::get_pending_annotation,
      commands::get_annotation_config,
      commands::save_annotated_capture,
      commands::cancel_annotation,
      commands::validate_save_folder,
      commands::toggle_recording,
      commands::stop_recording,
      commands::start_region_recording,
      commands::get_recording_state,
      commands::reveal_file,
      commands::annotate_file,
      commands::get_monitor_list,
      commands::complete_select_screen_capture,
      commands::complete_ocr_capture,
      commands::ocr_image,
      commands::upload_last_to_imgur,
    ])
    .setup(|app| {
      let config = config::load_config(&app.handle());
      let launch_on_startup = config.launch_on_startup;
      let explorer_ctx_menu = config.explorer_context_menu;
      app.manage(Mutex::new(state::AppState::new(config)));

      // Sync autostart registry/launch-agent to match config on every launch.
      // Covers the case where the entry was removed externally (reinstall,
      // system cleanup, etc.) while the config still says enabled.
      startup::set_launch_on_startup(&app.handle(), launch_on_startup);
      if explorer_ctx_menu {
        context_menu::set_context_menu(true);
      }

      tray::setup_tray(&app.handle())?;

      // Handle CLI args on initial launch (e.g. "quickshotter.exe --annotate photo.png")
      let args: Vec<String> = std::env::args().collect();
      if args.len() > 1 {
        let handle = app.handle().clone();
        // Defer to async so the app finishes setup first
        tauri::async_runtime::spawn(async move {
          handle_cli_args(&handle, &args[1..]);
        });
      }

      if let Err(e) = hotkeys::register_hotkeys(&app.handle()) {
        eprintln!("Failed to register hotkeys: {e}");
        use tauri_plugin_notification::NotificationExt;
        #[cfg(target_os = "macos")]
        let body = format!("Failed to register hotkeys: {}\nCheck System Settings > Privacy & Security > Accessibility", e);
        #[cfg(not(target_os = "macos"))]
        let body = format!("Failed to register hotkeys: {}", e);
        app.handle().notification()
          .builder()
          .title("QuickShotter")
          .body(&body)
          .show()
          .ok();
      }

      Ok(())
    })
    .on_window_event(|window, event| {
      // Safety net: if a window is destroyed without going through its normal
      // close path (JS crash, Alt+F4, system kill), reset the corresponding
      // state flag so the app doesn't get permanently stuck.
      if let tauri::WindowEvent::Destroyed = event {
        let label = window.label();
        let s = window.app_handle().state::<Mutex<state::AppState>>();
        if label == "overlay" {
          window_capture::stop();
          let mut state = s.lock_or_recover();
          state.is_capturing = false;
          state.pending_screenshot = None;
          state.pending_base64 = None;
          state.overlay_window_id = 0;
        } else if label == "annotation" {
          let mut state = s.lock_or_recover();
          state.is_annotating = false;
          state.pending_annotation = None;
          state.pending_annotation_base64 = None;
          state.annotation_source_path = None;
        }
      }
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
