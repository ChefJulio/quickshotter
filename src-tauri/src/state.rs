use image::RgbaImage;
use std::path::PathBuf;

use crate::config::AppConfig;

pub const MAX_HISTORY: usize = 5;

pub struct AppState {
  pub config: AppConfig,
  pub capture_history: Vec<PathBuf>,
  pub is_capturing: bool,
  // Only used in "freeze" / "window" capture modes
  pub pending_screenshot: Option<RgbaImage>,
  pub pending_base64: Option<String>,
  // Last saved file, used for notification click -> reveal in explorer
  pub last_saved_path: Option<PathBuf>,
  // Overlay mode: "instant", "freeze", or "window"
  pub overlay_mode: String,
  // Window ID of the overlay (used to exclude from window detection)
  pub overlay_window_id: u32,
  // Annotation: image waiting to be annotated
  pub pending_annotation: Option<RgbaImage>,
  pub pending_annotation_base64: Option<String>,
}

impl AppState {
  pub fn new(config: AppConfig) -> Self {
    Self {
      config,
      capture_history: Vec::new(),
      is_capturing: false,
      pending_screenshot: None,
      pending_base64: None,
      last_saved_path: None,
      overlay_mode: "instant".to_string(),
      overlay_window_id: 0,
      pending_annotation: None,
      pending_annotation_base64: None,
    }
  }

  pub fn add_to_history(&mut self, path: PathBuf) {
    self.capture_history.push(path);
    if self.capture_history.len() > MAX_HISTORY {
      self.capture_history.remove(0);
    }
  }
}
