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
  #[serde(default = "default_hotkey_window")]
  pub hotkey_window: String,
  pub format: String,
  pub filename_prefix: String,
  pub save_to_disk: bool,
  #[serde(default = "default_capture_mode")]
  pub capture_mode: String,
  #[serde(default = "default_true")]
  pub copy_to_clipboard: bool,
  #[serde(default)]
  pub annotate_captures: bool,
  #[serde(default = "default_shift_tool")]
  pub annotate_shift_tool: String,
  #[serde(default = "default_ctrl_tool")]
  pub annotate_ctrl_tool: String,
  #[serde(default = "default_alt_tool")]
  pub annotate_alt_tool: String,
  #[serde(default = "default_annotate_default_tool")]
  pub annotate_default_tool: String,
  #[serde(default = "default_filename_suffix")]
  pub filename_suffix: String,
  #[serde(default)]
  pub launch_on_startup: bool,
  // Recording settings
  #[serde(default = "default_hotkey_record")]
  pub hotkey_record: String,
  #[serde(default = "default_recording_format")]
  pub recording_format: String,
  #[serde(default = "default_recording_fps")]
  pub recording_fps: u32,
  #[serde(default = "default_gif_max_duration")]
  pub gif_max_duration: u32,
  #[serde(default = "default_gif_max_width")]
  pub gif_max_width: u32,
}

fn default_true() -> bool {
  true
}

fn default_annotate_default_tool() -> String {
  "freehand".to_string()
}

fn default_filename_suffix() -> String {
  "%Y-%m-%d_%H-%M-%S".to_string()
}

fn default_capture_mode() -> String {
  "instant".to_string()
}

fn default_hotkey_window() -> String {
  "CmdOrCtrl+Alt+Shift+W".to_string()
}

fn default_hotkey_record() -> String {
  "CmdOrCtrl+Alt+Shift+R".to_string()
}

fn default_recording_format() -> String {
  "mp4".to_string()
}

fn default_recording_fps() -> u32 {
  30
}

fn default_gif_max_duration() -> u32 {
  15
}

fn default_gif_max_width() -> u32 {
  800
}

fn default_shift_tool() -> String {
  "arrow".to_string()
}

fn default_ctrl_tool() -> String {
  "oval".to_string()
}

fn default_alt_tool() -> String {
  "text".to_string()
}

impl Default for AppConfig {
  fn default() -> Self {
    Self {
      save_folder: default_save_folder(),
      hotkey_region: "CmdOrCtrl+Alt+Shift+S".to_string(),
      hotkey_fullscreen: "CmdOrCtrl+Alt+Shift+D".to_string(),
      hotkey_window: default_hotkey_window(),
      format: "png".to_string(),
      filename_prefix: "screenshot".to_string(),
      save_to_disk: true,
      copy_to_clipboard: true,
      capture_mode: "instant".to_string(),
      annotate_captures: false,
      annotate_shift_tool: default_shift_tool(),
      annotate_ctrl_tool: default_ctrl_tool(),
      annotate_alt_tool: default_alt_tool(),
      annotate_default_tool: default_annotate_default_tool(),
      filename_suffix: default_filename_suffix(),
      launch_on_startup: false,
      hotkey_record: default_hotkey_record(),
      recording_format: default_recording_format(),
      recording_fps: default_recording_fps(),
      gif_max_duration: default_gif_max_duration(),
      gif_max_width: default_gif_max_width(),
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

  // Atomic write: write to a temp file in the same directory, then rename.
  // This prevents corruption if the process crashes mid-write.
  let tmp_path = path.with_extension("json.tmp");
  fs::write(&tmp_path, content + "\n")?;
  fs::rename(&tmp_path, &path)?;
  Ok(())
}
