//! Window creation — fullscreen overlay.
//!
//! Two window types:
//! - Freeze mode: opaque window (wgpu renders screenshot to surface)
//! - Live mode: Win32 layered window with per-pixel alpha (real transparency)

use winit::event_loop::ActiveEventLoop;
use winit::window::{CursorIcon, Window, WindowAttributes, WindowLevel};

use crate::protocol::CaptureMode;

/// Create the overlay window spanning the virtual desktop.
pub fn create_overlay_window(
  event_loop: &ActiveEventLoop,
  origin_x: i32,
  origin_y: i32,
  bounds_w: u32,
  bounds_h: u32,
  mode: CaptureMode,
) -> Result<Window, String> {
  // Live modes: transparent window with per-pixel alpha (UpdateLayeredWindow).
  // Freeze mode: opaque window (wgpu renders to surface directly).
  let transparent = mode != CaptureMode::Freeze;

  // Freeze mode: start hidden, show after first GPU frame to avoid white flash.
  // Live modes: must be visible immediately for layered window APIs to work.
  let start_visible = mode != CaptureMode::Freeze;

  let attrs = WindowAttributes::default()
    .with_title("QuickShotter Overlay")
    .with_visible(start_visible)
    .with_transparent(transparent)
    .with_decorations(false)
    .with_resizable(false)
    .with_window_level(WindowLevel::AlwaysOnTop)
    .with_position(winit::dpi::PhysicalPosition::new(origin_x, origin_y))
    .with_inner_size(winit::dpi::PhysicalSize::new(bounds_w, bounds_h))
    .with_cursor(CursorIcon::Crosshair);

  let window = event_loop
    .create_window(attrs)
    .map_err(|e| format!("Failed to create window: {e}"))?;

  #[cfg(target_os = "windows")]
  apply_win32_fixups(&window, mode);

  Ok(window)
}

/// Win32-specific window style adjustments.
#[cfg(target_os = "windows")]
fn apply_win32_fixups(window: &Window, mode: CaptureMode) {
  use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};

  let handle = match window.window_handle() {
    Ok(h) => h,
    Err(_) => return,
  };

  let hwnd = match handle.as_raw() {
    RawWindowHandle::Win32(h) => h.hwnd.get() as isize,
    _ => return,
  };

  use windows::Win32::Foundation::HWND;
  use windows::Win32::UI::WindowsAndMessaging::{
    GetWindowLongW, SetWindowLongW, GWL_EXSTYLE,
    WS_EX_APPWINDOW, WS_EX_LAYERED, WS_EX_TOOLWINDOW,
  };

  unsafe {
    let ex_style = GetWindowLongW(HWND(hwnd as *mut _), GWL_EXSTYLE) as u32;
    let mut new_style = (ex_style & !WS_EX_APPWINDOW.0) | WS_EX_TOOLWINDOW.0;

    if mode != CaptureMode::Freeze {
      new_style |= WS_EX_LAYERED.0;
    }

    SetWindowLongW(HWND(hwnd as *mut _), GWL_EXSTYLE, new_style as i32);
  }
}

/// Get the HWND from a winit window.
#[cfg(target_os = "windows")]
pub fn get_hwnd(window: &Window) -> Option<isize> {
  use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
  let handle = window.window_handle().ok()?;
  match handle.as_raw() {
    RawWindowHandle::Win32(h) => Some(h.hwnd.get() as isize),
    _ => None,
  }
}
