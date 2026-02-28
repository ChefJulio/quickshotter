use std::path::PathBuf;
use std::sync::Mutex;
use tauri::{
  AppHandle, Manager,
  menu::{MenuBuilder, MenuItemBuilder, PredefinedMenuItem},
  tray::TrayIconBuilder,
};

use crate::state::AppState;

pub fn setup_tray(app: &AppHandle) -> Result<(), Box<dyn std::error::Error>> {
  let menu = build_tray_menu(app, &[])?;

  TrayIconBuilder::with_id("main")
    .icon(app.default_window_icon().unwrap().clone())
    .menu(&menu)
    .tooltip("QuickShotter")
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
          let app = app.clone();
          tauri::async_runtime::spawn(async move {
            if let Err(e) = crate::commands::do_fullscreen_capture(&app).await {
              eprintln!("Fullscreen capture failed: {e}");
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
          if id.starts_with("history_") {
            if let Ok(idx) = id.strip_prefix("history_").unwrap().parse::<usize>() {
              let state = app.state::<Mutex<AppState>>();
              let state = state.lock().unwrap();
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
) -> Result<tauri::menu::Menu<tauri::Wry>, Box<dyn std::error::Error>> {
  let mut builder = MenuBuilder::new(app);

  builder = builder.item(
    &MenuItemBuilder::with_id("capture_region", "Capture Region  (Ctrl+Alt+Shift+S)")
      .build(app)?,
  );
  builder = builder.item(
    &MenuItemBuilder::with_id("capture_window", "Capture Window  (Ctrl+Alt+Shift+W)")
      .build(app)?,
  );
  builder = builder.item(
    &MenuItemBuilder::with_id("capture_fullscreen", "Capture Fullscreen  (Ctrl+Alt+Shift+D)")
      .build(app)?,
  );

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
  builder = builder.item(&MenuItemBuilder::with_id("settings", "Settings").build(app)?);
  builder = builder.item(&MenuItemBuilder::with_id("exit", "Exit").build(app)?);

  Ok(builder.build()?)
}

pub fn refresh_tray_menu(app: &AppHandle) {
  let history = {
    let state = app.state::<Mutex<AppState>>();
    let state = state.lock().unwrap();
    state.capture_history.clone()
  };
  if let Ok(menu) = build_tray_menu(app, &history) {
    if let Some(tray) = app.tray_by_id("main") {
      tray.set_menu(Some(menu)).ok();
    }
  }
}

fn open_in_explorer(path: &PathBuf) {
  #[cfg(target_os = "windows")]
  {
    std::process::Command::new("explorer")
      .args(["/select,", &path.to_string_lossy()])
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
