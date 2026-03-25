use std::path::PathBuf;
use std::sync::Mutex;
use tauri::{
  AppHandle, Manager,
  menu::{MenuBuilder, MenuItemBuilder, PredefinedMenuItem},
  tray::{MouseButton, TrayIconBuilder, TrayIconEvent},
};

use crate::state::{AppState, LockRecover};

fn format_hotkey_display(raw: &str) -> String {
  #[cfg(target_os = "macos")]
  let result = raw.replace("CmdOrCtrl", "Cmd");
  #[cfg(not(target_os = "macos"))]
  let result = raw.replace("CmdOrCtrl", "Ctrl");
  result.replace("Backquote", "`")
}

pub fn setup_tray(app: &AppHandle) -> Result<(), Box<dyn std::error::Error>> {
  let config = app.state::<Mutex<AppState>>().lock_or_recover().config.clone();
  let menu = build_tray_menu(app, &[], &config)?;

  let mut tray = TrayIconBuilder::with_id("main");
  if let Some(icon) = app.default_window_icon() {
    tray = tray.icon(icon.clone());
  }
  tray.menu(&menu)
    .tooltip("QuickShotter")
    .show_menu_on_left_click(false)
    .on_tray_icon_event(|tray, event| {
      // Left-click tray icon -> open settings (standard Windows tray pattern).
      // Right-click still shows the context menu (default tray behavior).
      if let TrayIconEvent::Click { button: MouseButton::Left, .. } = event {
        crate::commands::show_settings_window(tray.app_handle());
      }
    })
    .on_menu_event(move |app, event| {
      match event.id().as_ref() {
        "capture_region" => {
          if let Err(e) = crate::overlay::open_overlay(&app) {
            eprintln!("Region capture failed: {e}");
          }
        }
        "capture_window" => {
          if let Err(e) = crate::overlay::open_overlay_with_mode(&app, "window") {
            eprintln!("Window capture failed: {e}");
          }
        }
        "capture_fullscreen" => {
          let fs_mode = app.state::<std::sync::Mutex<crate::state::AppState>>()
            .lock_or_recover().config.fullscreen_mode.clone();
          if fs_mode == "select" {
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
        "record_screen" => {
          let app = app.clone();
          tauri::async_runtime::spawn(async move {
            if let Err(e) = crate::commands::toggle_recording(app).await {
              eprintln!("Recording toggle failed: {e}");
            }
          });
        }
        "record_region" => {
          if let Err(e) = crate::overlay::open_overlay_with_mode(&app, "record_region") {
            eprintln!("Record region overlay failed: {e}");
          }
        }
        "stop_recording" => {
          let app = app.clone();
          tauri::async_runtime::spawn(async move {
            if let Err(e) = crate::commands::stop_recording(app).await {
              eprintln!("Stop recording failed: {e}");
            }
          });
        }
        "annotate_file" => {
          let app = app.clone();
          tauri::async_runtime::spawn(async move {
            use tauri_plugin_dialog::DialogExt;
            if let Some(path) = app.dialog().file()
              .add_filter("Images", &["png", "jpg", "jpeg", "webp", "bmp"])
              .blocking_pick_file()
            {
              if let Err(e) = crate::commands::annotate_file_from_path(&app, &std::path::PathBuf::from(path.to_string())) {
                eprintln!("Annotate file failed: {e}");
              }
            }
          });
        }
        "settings" => {
          crate::commands::show_settings_window(&app);
        }
        "exit" => {
          app.exit(0);
        }
        id => {
          // History items: "history_0", "history_1", etc.
          if let Some(suffix) = id.strip_prefix("history_") {
            if let Ok(idx) = suffix.parse::<usize>() {
              let s = app.state::<Mutex<AppState>>();
              let state = s.lock_or_recover();
              if let Some(path) = state.capture_history.get(idx) {
                open_in_explorer(path);
              }
            }
          }
        }
      }
    })
    .build(app)?;

  Ok(())
}

pub fn build_tray_menu(
  app: &AppHandle,
  history: &[PathBuf],
  config: &crate::config::AppConfig,
) -> Result<tauri::menu::Menu<tauri::Wry>, Box<dyn std::error::Error>> {
  let mut builder = MenuBuilder::new(app);

  let region_label = format!("Capture Region  ({})", format_hotkey_display(&config.hotkey_region));
  let window_label = format!("Capture Window  ({})", format_hotkey_display(&config.hotkey_window));
  let fullscreen_label = format!("Capture Fullscreen  ({})", format_hotkey_display(&config.hotkey_fullscreen));

  builder = builder.item(
    &MenuItemBuilder::with_id("capture_region", &region_label)
      .build(app)?,
  );
  builder = builder.item(
    &MenuItemBuilder::with_id("capture_window", &window_label)
      .build(app)?,
  );
  builder = builder.item(
    &MenuItemBuilder::with_id("capture_fullscreen", &fullscreen_label)
      .build(app)?,
  );

  builder = builder.item(&PredefinedMenuItem::separator(app)?);

  // Recording items: show "Stop Recording" when active, otherwise show "Record Screen"
  let is_recording = app.state::<Mutex<AppState>>().lock_or_recover().is_recording;
  if is_recording {
    builder = builder.item(
      &MenuItemBuilder::with_id("stop_recording", "Stop Recording")
        .build(app)?,
    );
  } else {
    let record_region_label = format!("Record Region  ({})", format_hotkey_display(&config.hotkey_record));
    builder = builder.item(
      &MenuItemBuilder::with_id("record_region", &record_region_label)
        .build(app)?,
    );
    builder = builder.item(
      &MenuItemBuilder::with_id("record_screen", "Record Fullscreen")
        .build(app)?,
    );
  }

  if !history.is_empty() {
    builder = builder.item(&PredefinedMenuItem::separator(app)?);
    for (i, path) in history.iter().rev().enumerate() {
      let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| format!("Capture {}", i + 1));
      builder = builder.item(
        &MenuItemBuilder::with_id(format!("history_{}", history.len() - 1 - i), &name)
          .build(app)?,
      );
    }
  }

  builder = builder.item(&PredefinedMenuItem::separator(app)?);
  builder = builder.item(&MenuItemBuilder::with_id("annotate_file", "Annotate File...").build(app)?);
  builder = builder.item(&MenuItemBuilder::with_id("settings", "Settings").build(app)?);
  builder = builder.item(&MenuItemBuilder::with_id("exit", "Exit").build(app)?);

  Ok(builder.build()?)
}

pub fn refresh_tray_menu(app: &AppHandle) {
  let (history, config) = {
    let s = app.state::<Mutex<AppState>>();
    let state = s.lock_or_recover();
    let history: Vec<PathBuf> = state.capture_history.iter().cloned().collect();
    (history, state.config.clone())
  };
  if let Ok(menu) = build_tray_menu(app, &history, &config) {
    if let Some(tray) = app.tray_by_id("main") {
      if let Err(e) = tray.set_menu(Some(menu)) {
        eprintln!("Failed to update tray menu: {e}");
      }
    }
  }
}

pub fn open_in_explorer(path: &PathBuf) {
  #[cfg(target_os = "windows")]
  {
    // Use raw_arg so the /select, and path are passed as one token to explorer
    // without extra quoting from Rust's Command::arg().
    use std::os::windows::process::CommandExt;
    std::process::Command::new("explorer")
      .raw_arg(format!("/select,\"{}\"", path.display()))
      .spawn()
      .map_err(|e| eprintln!("Failed to open explorer: {e}"))
      .ok();
  }
  #[cfg(target_os = "macos")]
  {
    std::process::Command::new("open")
      .args(["-R", &path.to_string_lossy()])
      .spawn()
      .map_err(|e| eprintln!("Failed to open Finder: {e}"))
      .ok();
  }
}
