use std::path::PathBuf;
use std::sync::Mutex;
use tauri::{
  AppHandle, Manager,
  menu::{MenuBuilder, MenuItemBuilder, PredefinedMenuItem, SubmenuBuilder},
  tray::{MouseButton, TrayIconBuilder, TrayIconEvent},
};

use crate::state::{AppState, LockRecover};

fn format_hotkey_display(raw: &str) -> String {
  #[cfg(target_os = "macos")]
  let result = raw
    .replace("CmdOrCtrl", "⌘")
    .replace("Alt", "⌥")
    .replace("Shift", "⇧")
    .replace("Backquote", "`")
    .replace("+", "");
  #[cfg(not(target_os = "macos"))]
  let result = raw.replace("CmdOrCtrl", "Ctrl").replace("Backquote", "`");
  result
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
    .show_menu_on_left_click(true)
    .on_tray_icon_event(|_tray, _event| {
      // Menu shows on both left and right click (Tauri default).
      // Settings is accessible from the menu.
    })
    .on_menu_event(move |app, event| {
      match event.id().as_ref() {
        "capture_region" => {
          crate::commands::delayed_region_capture(&app);
        }
        "capture_window" => {
          crate::commands::delayed_window_capture(&app);
        }
        "capture_fullscreen" => {
          crate::commands::delayed_fullscreen_capture(&app);
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
        "ocr_region" => {
          crate::commands::delayed_ocr_capture(&app);
        }
        "upload_imgur" => {
          let app = app.clone();
          tauri::async_runtime::spawn(async move {
            if let Err(e) = crate::commands::upload_last_to_imgur(app.clone()).await {
              eprintln!("Upload failed: {e}");
              use tauri_plugin_notification::NotificationExt;
              app.notification()
                .builder()
                .title("Upload failed")
                .body(&e.to_string())
                .show()
                .ok();
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

  // Capture submenu
  let capture_sub = SubmenuBuilder::with_id(app, "capture_menu", "Capture")
    .item(&MenuItemBuilder::with_id("capture_region",
      &format!("Region  ({})", format_hotkey_display(&config.hotkey_region))).build(app)?)
    .item(&MenuItemBuilder::with_id("capture_window",
      &format!("Window  ({})", format_hotkey_display(&config.hotkey_window))).build(app)?)
    .item(&MenuItemBuilder::with_id("capture_fullscreen",
      &format!("Screen  ({})", format_hotkey_display(&config.hotkey_fullscreen))).build(app)?)
    .item(&MenuItemBuilder::with_id("ocr_region",
      &format!("Text (OCR)  ({})", format_hotkey_display(&config.hotkey_ocr))).build(app)?)
    .build()?;
  builder = builder.item(&capture_sub);

  // Record submenu
  let is_recording = app.state::<Mutex<AppState>>().lock_or_recover().is_recording;
  let record_sub = if is_recording {
    SubmenuBuilder::with_id(app, "record_menu", "Record")
      .item(&MenuItemBuilder::with_id("stop_recording", "Stop Recording").build(app)?)
      .build()?
  } else {
    SubmenuBuilder::with_id(app, "record_menu", "Record")
      .item(&MenuItemBuilder::with_id("record_region",
        &format!("Region  ({})", format_hotkey_display(&config.hotkey_record))).build(app)?)
      .item(&MenuItemBuilder::with_id("record_screen", "Fullscreen").build(app)?)
      .build()?
  };
  builder = builder.item(&record_sub);

  // Recent captures submenu
  if !history.is_empty() {
    let mut recent = SubmenuBuilder::with_id(app, "recent_menu", "Recent");
    for (i, path) in history.iter().rev().enumerate() {
      let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| format!("Capture {}", i + 1));
      recent = recent.item(
        &MenuItemBuilder::with_id(format!("history_{}", history.len() - 1 - i), &name)
          .build(app)?,
      );
    }
    builder = builder.item(&recent.build()?);
  }

  builder = builder.item(&PredefinedMenuItem::separator(app)?);
  builder = builder.item(&MenuItemBuilder::with_id("upload_imgur", "Upload to catbox.moe").build(app)?);
  builder = builder.item(&MenuItemBuilder::with_id("annotate_file", "Annotate File...").build(app)?);
  builder = builder.item(&MenuItemBuilder::with_id("settings", "Settings").build(app)?);
  builder = builder.item(&MenuItemBuilder::with_id("exit", "Exit").build(app)?);

  Ok(builder.build()?)
}

pub fn refresh_tray_menu(app: &AppHandle) {
  let (history, config) = {
    let s = app.state::<Mutex<AppState>>();
    let state = s.lock_or_recover();
    // Clone only what build_tray_menu needs (5 hotkey strings + history paths)
    let history: Vec<PathBuf> = state.capture_history.iter().cloned().collect();
    let config = crate::config::AppConfig {
      hotkey_region: state.config.hotkey_region.clone(),
      hotkey_window: state.config.hotkey_window.clone(),
      hotkey_fullscreen: state.config.hotkey_fullscreen.clone(),
      hotkey_ocr: state.config.hotkey_ocr.clone(),
      hotkey_record: state.config.hotkey_record.clone(),
      ..Default::default()
    };
    (history, config)
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
