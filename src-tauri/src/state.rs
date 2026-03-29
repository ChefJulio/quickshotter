use image::RgbaImage;
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};
use std::sync::atomic::AtomicBool;

use crate::config::AppConfig;
use crate::recording::{RecordingRegion, pipeline::PipelineHandle};

pub const MAX_HISTORY: usize = 5;

pub struct AppState {
  pub config: AppConfig,
  pub capture_history: VecDeque<PathBuf>,
  pub is_capturing: bool,
  pub is_annotating: bool,
  // Only used in "freeze" / "window" capture modes
  pub pending_screenshot: Option<RgbaImage>,
  pub pending_base64: Option<String>,
  // Last saved file, used for notification click -> reveal in explorer
  pub last_saved_path: Option<PathBuf>,
  // Overlay mode: "instant", "freeze", or "window"
  pub overlay_mode: String,
  // Desktop bounds cached when overlay opens (avoids re-enumerating monitors)
  pub cached_bounds: Option<(i32, i32, u32, u32)>,
  // Window ID of the overlay (used to exclude from window detection)
  pub overlay_window_id: u32,
  // Annotation: image waiting to be annotated
  pub pending_annotation: Option<RgbaImage>,
  // Temp file path for annotation image (loaded via asset protocol, not base64)
  pub pending_annotation_path: Option<PathBuf>,
  // When annotating a file from disk, stores the source path for save-beside behavior
  pub annotation_source_path: Option<PathBuf>,
  // Delayed capture: saved params while countdown window is showing
  pub delayed_capture: Option<serde_json::Value>,
  // Recording state
  pub is_recording: bool,
  pub recording_stop_signal: Option<Arc<AtomicBool>>,
  pub recording_region: Option<RecordingRegion>,
  pub recording_start: Option<std::time::Instant>,
  pub pipeline_handle: Option<PipelineHandle>,
}

impl AppState {
  pub fn new(config: AppConfig) -> Self {
    Self {
      config,
      capture_history: VecDeque::new(),
      is_capturing: false,
      is_annotating: false,
      pending_screenshot: None,
      pending_base64: None,
      last_saved_path: None,
      overlay_mode: "instant".to_string(),
      cached_bounds: None,
      overlay_window_id: 0,
      pending_annotation: None,
      pending_annotation_path: None,
      annotation_source_path: None,
      delayed_capture: None,
      is_recording: false,
      recording_stop_signal: None,
      recording_region: None,
      recording_start: None,
      pipeline_handle: None,
    }
  }

  pub fn add_to_history(&mut self, path: PathBuf) {
    self.capture_history.push_back(path);
    while self.capture_history.len() > MAX_HISTORY {
      self.capture_history.pop_front();
    }
  }
}

/// Extension trait for Mutex that recovers from poisoning instead of panicking.
pub trait LockRecover<T> {
  fn lock_or_recover(&self) -> MutexGuard<'_, T>;
}

impl<T> LockRecover<T> for Mutex<T> {
  fn lock_or_recover(&self) -> MutexGuard<'_, T> {
    self.lock().unwrap_or_else(|poisoned| {
      eprintln!("Mutex was poisoned, recovering");
      poisoned.into_inner()
    })
  }
}
