use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use tauri::{AppHandle, Manager};

use crate::error::AppError;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
  pub save_folder: String,
  pub hotkey_region: String,
  pub hotkey_fullscreen: String,
  pub format: String,
  pub filename_prefix: String,
  pub save_to_disk: bool,
  #[serde(default = "default_capture_mode")]
  pub capture_mode: String,
}

fn default_capture_mode() -> String {
  "instant".to_string()
}

impl Default for AppConfig {
  fn default() -> Self {
    Self {
      save_folder: default_save_folder(),
      hotkey_region: "CmdOrCtrl+Alt+Shift+S".to_string(),
      hotkey_fullscreen: "CmdOrCtrl+Alt+Shift+D".to_string(),
      format: "jpg".to_string(),
      filename_prefix: "quickshotter".to_string(),
      save_to_disk: true,
      capture_mode: "instant".to_string(),
    }
  }
}

fn default_save_folder() -> String {
  if let Some(pic_dir) = dirs::picture_dir() {
    let screenshots = pic_dir.join("Screenshots");
    return screenshots.to_string_lossy().to_string();
  }
  if let Some(home) = dirs::home_dir() {
    return home.join("Pictures").join("Screenshots").to_string_lossy().to_string();
  }
  "~/Pictures/Screenshots".to_string()
}

pub fn config_path(app: &AppHandle) -> PathBuf {
  let config_dir = app.path().app_config_dir().expect("failed to get app config dir");
  fs::create_dir_all(&config_dir).ok();
  config_dir.join("config.json")
}

pub fn load_config(app: &AppHandle) -> AppConfig {
  let path = config_path(app);
  if path.exists() {
    match fs::read_to_string(&path) {
      Ok(content) => match serde_json::from_str(&content) {
        Ok(config) => return config,
        Err(e) => eprintln!("Failed to parse config: {e}"),
      },
      Err(e) => eprintln!("Failed to read config: {e}"),
    }
  }
  let config = AppConfig::default();
  save_config(app, &config).ok();
  config
}

pub fn save_config(app: &AppHandle, config: &AppConfig) -> Result<(), AppError> {
  let path = config_path(app);
  let content = serde_json::to_string_pretty(config)
    .map_err(|e| AppError::Config(e.to_string()))?;
  fs::write(&path, content + "\n")?;
  Ok(())
}
