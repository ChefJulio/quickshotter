//! Mouse / keyboard state machine for all capture modes.

use crate::protocol::CaptureMode;

/// Current interaction state.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Phase {
  /// Waiting for user input.
  Idle,
  /// Mouse is held down, dragging a selection rectangle (region mode).
  Dragging,
  /// Selection complete.
  Done,
  /// User cancelled.
  Cancelled,
}

/// A rectangle in physical pixels (screen-space or window-local depending on context).
#[derive(Debug, Clone, Copy)]
pub struct Rect {
  pub x1: f32,
  pub y1: f32,
  pub x2: f32,
  pub y2: f32,
}

/// Monitor info for select_screen mode.
#[derive(Debug, Clone)]
pub struct MonitorRect {
  /// Screen-space bounds.
  pub x: i32,
  pub y: i32,
  pub w: u32,
  pub h: u32,
  pub index: usize,
}

/// Tracks all interaction modes.
pub struct Interaction {
  pub mode: CaptureMode,
  pub phase: Phase,

  // -- Mouse state --
  pub start_x: f32,
  pub start_y: f32,
  pub current_x: f32,
  pub current_y: f32,
  pub shift: bool,
  pub alt: bool,

  // -- Window mode --
  /// Current window highlight (window-local coords).
  pub hovered_window: Option<Rect>,
  /// The selected window (screen-space coords, set on click).
  pub selected_window: Option<(i32, i32, i32, i32)>,

  // -- Select screen mode --
  /// All monitors, with bounds converted to window-local coords.
  pub monitors: Vec<MonitorRect>,
  /// Index of the hovered monitor.
  pub hovered_monitor: Option<usize>,
  /// Selected monitor index.
  pub selected_monitor: Option<usize>,
}

impl Interaction {
  pub fn new(mode: CaptureMode) -> Self {
    Self {
      mode,
      phase: Phase::Idle,
      start_x: 0.0,
      start_y: 0.0,
      current_x: 0.0,
      current_y: 0.0,
      shift: false,
      alt: false,
      hovered_window: None,
      selected_window: None,
      monitors: Vec::new(),
      hovered_monitor: None,
      selected_monitor: None,
    }
  }

  // ── Region mode ──────────────────────────────────────

  pub fn mouse_down(&mut self, x: f32, y: f32) {
    match self.mode {
      CaptureMode::Window => {
        // Click = select the hovered window (or last detected window)
        if self.hovered_window.is_some() || self.selected_window.is_some() {
          self.phase = Phase::Done;
        }
      }
      CaptureMode::SelectScreen => {
        if self.hovered_monitor.is_some() {
          self.selected_monitor = self.hovered_monitor;
          self.phase = Phase::Done;
        }
      }
      _ => {
        // Region modes
        if self.phase != Phase::Idle {
          return;
        }
        self.start_x = x;
        self.start_y = y;
        self.current_x = x;
        self.current_y = y;
        self.phase = Phase::Dragging;
      }
    }
  }

  pub fn mouse_move(&mut self, x: f32, y: f32) {
    self.current_x = x;
    self.current_y = y;
  }

  pub fn mouse_up(&mut self, x: f32, y: f32, shift: bool, alt: bool) {
    self.shift = shift;
    self.alt = alt;

    match self.mode {
      CaptureMode::Window | CaptureMode::SelectScreen => {
        // Click-based, not drag-based — handled in mouse_down
      }
      _ => {
        if self.phase != Phase::Dragging {
          return;
        }
        self.current_x = x;
        self.current_y = y;

        let w = (self.current_x - self.start_x).abs();
        let h = (self.current_y - self.start_y).abs();
        if w < 3.0 || h < 3.0 {
          self.phase = Phase::Cancelled;
        } else {
          self.phase = Phase::Done;
        }
      }
    }
  }

  pub fn cancel(&mut self) {
    self.phase = Phase::Cancelled;
  }

  // ── Window mode ──────────────────────────────────────

  /// Update the hovered window highlight. Called from the event loop
  /// with the result from the window detection thread.
  pub fn set_hovered_window(&mut self, rect: Option<Rect>) {
    self.hovered_window = rect;
  }

  /// Set the selected window bounds (screen-space).
  pub fn set_selected_window(&mut self, left: i32, top: i32, right: i32, bottom: i32) {
    self.selected_window = Some((left, top, right, bottom));
  }

  // ── Select screen mode ───────────────────────────────

  /// Set up the monitor list. Bounds are in screen-space.
  pub fn set_monitors(&mut self, monitors: Vec<MonitorRect>) {
    self.monitors = monitors;
  }

  /// Update the hovered monitor based on cursor position.
  /// Both cursor and monitor bounds are in window-local coordinates.
  pub fn update_hovered_monitor(&mut self) {
    let cx = self.current_x as i32;
    let cy = self.current_y as i32;

    let old = self.hovered_monitor;
    self.hovered_monitor = self.monitors.iter().position(|m| {
      cx >= m.x && cx < m.x + m.w as i32
        && cy >= m.y && cy < m.y + m.h as i32
    });
    if self.hovered_monitor != old {
      if let Some(idx) = self.hovered_monitor {
        let m = &self.monitors[idx];
        eprintln!("[select-screen] hover: vec_idx={} xcap_idx={} bounds=({},{} {}x{})", idx, m.index, m.x, m.y, m.w, m.h);
      }
    }
  }

  // ── Results ──────────────────────────────────────────

  /// Region selection in screen-space physical pixels.
  pub fn selection(&self) -> Option<(f32, f32, f32, f32)> {
    if self.phase != Phase::Dragging && self.phase != Phase::Done {
      return None;
    }
    if matches!(self.mode, CaptureMode::Window | CaptureMode::SelectScreen) {
      return None;
    }
    let x1 = self.start_x.min(self.current_x);
    let y1 = self.start_y.min(self.current_y);
    let x2 = self.start_x.max(self.current_x);
    let y2 = self.start_y.max(self.current_y);
    Some((x1, y1, x2, y2))
  }

  /// Final region result in screen coordinates.
  /// Coordinates are already in screen-space (callers convert window-local to
  /// screen-space before updating the interaction).
  pub fn region_result(&self) -> Option<(i32, i32, i32, i32, bool, bool)> {
    if self.phase != Phase::Done { return None; }
    let (x1, y1, x2, y2) = self.selection()?;
    Some((
      x1 as i32,
      y1 as i32,
      x2 as i32,
      y2 as i32,
      self.shift,
      self.alt,
    ))
  }

  /// Final window result (screen-space bounds).
  pub fn window_result(&self) -> Option<(i32, i32, i32, i32, bool, bool)> {
    if self.phase != Phase::Done { return None; }
    let (left, top, right, bottom) = self.selected_window?;
    Some((left, top, right, bottom, self.shift, self.alt))
  }

  /// Final select_screen result (xcap monitor index).
  pub fn screen_result(&self) -> Option<usize> {
    if self.phase != Phase::Done { return None; }
    let vec_idx = self.selected_monitor?;
    // Return the original xcap monitor index, not the vec position
    Some(self.monitors.get(vec_idx)?.index)
  }
}
