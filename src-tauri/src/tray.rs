use std::path::PathBuf;
use std::sync::Mutex;
use tauri::{
  AppHandle, Manager,
  menu::{MenuBuilder, MenuItemBuilder, PredefinedMenuItem},
  tray::TrayIconBuilder,
};

use crate::state::{AppState, LockRecover};

fn format_hotkey_display(raw: &str) -> String {
  raw.replace("CmdOrCtrl", "Ctrl")
}

pub fn setup_tray(app: &AppHandle) -> Result<(), Box<dyn std::error::Error>> {
  let config = app.state::<Mutex<AppState>>().lock_or_recover().config.clone();
  let menu = build_tray_menu(app, &[], &config)?;

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

fn open_in_explorer(path: &PathBuf) {
  #[cfg(target_os = "windows")]
  {
    std::process::Command::new("explorer")
      .args(["/select,", &path.to_string_lossy()])
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
  #[cfg(target_os = "linux")]
  {
    if let Some(parent) = path.parent() {
      std::process::Command::new("xdg-open")
        .arg(parent)
        .spawn()
        .map_err(|e| eprintln!("Failed to open file manager: {e}"))
        .ok();
    }
  }
}
