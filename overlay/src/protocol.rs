//! Stdin/stdout JSON protocol for communication with the Tauri host.

use serde::{Deserialize, Serialize};

// ── Inbound (stdin, from Tauri) ────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Command {
  Capture(CaptureRequest),
  /// Show a click-through border around a recording region.
  ShowBorder(BorderRequest),
  /// Hide the recording border.
  HideBorder,
  Cancel,
  Quit,
}

#[derive(Debug, Deserialize)]
pub struct BorderRequest {
  pub x: i32,
  pub y: i32,
  pub width: u32,
  pub height: u32,
  pub origin_x: i32,
  pub origin_y: i32,
  pub bounds_w: u32,
  pub bounds_h: u32,
}

#[derive(Debug, Deserialize)]
pub struct CaptureRequest {
  pub mode: CaptureMode,
  /// Path to raw RGBA temp file (freeze mode).
  #[serde(default)]
  pub image: Option<String>,
  #[serde(default)]
  pub image_width: u32,
  #[serde(default)]
  pub image_height: u32,
  /// Virtual desktop origin X (physical pixels).
  #[serde(default)]
  pub origin_x: i32,
  #[serde(default)]
  pub origin_y: i32,
  /// Virtual desktop width (physical pixels).
  pub bounds_w: u32,
  /// Virtual desktop height (physical pixels).
  pub bounds_h: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureMode {
  #[serde(alias = "instant")] // Accept old config value
  Live,
  Freeze,
  Window,
  RecordRegion,
  Ocr,
  SelectScreen,
}

// ── Outbound (stdout, to Tauri) ────────────────────────────────────

#[derive(Debug, Serialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum OverlayResult {
  /// Daemon GPU initialization complete.
  #[serde(rename = "ready")]
  Ready,

  /// Region selected (freeze, live, ocr modes).
  /// Coordinates are logical screen points (macOS) or physical pixels (Windows).
  /// Can be negative (monitors left of / above the primary).
  Region {
    x1: i32,
    y1: i32,
    x2: i32,
    y2: i32,
    shift: bool,
    alt: bool,
    /// Freeze mode: path to raw RGBA file containing the cropped capture.
    /// When present, the host loads this directly instead of cropping from
    /// a pre-captured desktop image.
    #[serde(skip_serializing_if = "Option::is_none")]
    crop_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    crop_width: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    crop_height: Option<u32>,
  },

  /// Window selected (window mode).
  Window {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
    shift: bool,
    alt: bool,
  },

  /// Recording region selected.
  RecordRegion {
    x: i32,
    y: i32,
    width: u32,
    height: u32,
  },

  /// Monitor selected (select_screen mode).
  SelectScreen {
    monitor_index: u32,
  },

  /// User cancelled (Escape, right-click, or cancel command).
  Cancelled,
}

impl OverlayResult {
  /// Serialize and print to stdout as a single JSON line.
  pub fn send(&self) {
    if let Ok(json) = serde_json::to_string(self) {
      println!("{json}");
    }
  }
}
