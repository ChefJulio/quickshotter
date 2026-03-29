//! Stdin/stdout JSON protocol for communication with the Tauri host.

use serde::{Deserialize, Serialize};

// ── Inbound (stdin, from Tauri) ────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Command {
  Capture(CaptureRequest),
  Cancel,
  Quit,
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
  Instant,
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

  /// Region selected (freeze, instant, ocr modes).
  /// Coordinates are physical screen pixels and can be negative
  /// (monitors left of / above the primary).
  Region {
    x1: i32,
    y1: i32,
    x2: i32,
    y2: i32,
    shift: bool,
    alt: bool,
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
