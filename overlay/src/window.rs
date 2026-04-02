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

  // GPU-rendered modes start hidden, shown after first frame to avoid white flash.
  // On Windows, live modes use layered window APIs that need immediate visibility.
  #[cfg(target_os = "macos")]
  let start_visible = false;
  #[cfg(not(target_os = "macos"))]
  let start_visible = mode != CaptureMode::Freeze;

  // macOS: xcap returns logical points, and Tauri/winit expect logical coords.
  // Windows: coordinates are physical pixels.
  #[cfg(target_os = "macos")]
  let attrs = WindowAttributes::default()
    .with_title("QuickShotter Overlay")
    .with_visible(start_visible)
    .with_transparent(transparent)
    .with_decorations(false)
    .with_resizable(false)
    .with_window_level(WindowLevel::AlwaysOnTop)
    .with_position(winit::dpi::LogicalPosition::new(origin_x, origin_y))
    .with_inner_size(winit::dpi::LogicalSize::new(bounds_w, bounds_h))
    .with_cursor(CursorIcon::Crosshair);

  #[cfg(not(target_os = "macos"))]
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

  #[cfg(target_os = "macos")]
  apply_macos_fixups(&window);

  Ok(window)
}

/// macOS: ensure the overlay window can receive mouse/key events.
/// Borderless transparent windows need explicit activation + key window status.
#[cfg(target_os = "macos")]
fn apply_macos_fixups(window: &Window) {
  use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
  use objc2::runtime::AnyObject;
  use objc2::msg_send;
  use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy};

  let handle = match window.window_handle() {
    Ok(h) => h,
    Err(_) => return,
  };

  let ns_view = match handle.as_raw() {
    RawWindowHandle::AppKit(h) => h.ns_view.as_ptr() as *mut AnyObject,
    _ => return,
  };

  unsafe {
    // Get NSWindow from NSView
    let ns_window: *mut AnyObject = msg_send![ns_view, window];
    if ns_window.is_null() { return; }

    // Activate the application so it can receive events
    let mtm = objc2::MainThreadMarker::new_unchecked();
    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
    let _: () = msg_send![&*app, activateIgnoringOtherApps: true];

    // Ensure transparent compositing works
    let _: () = msg_send![ns_window, setOpaque: false];
    // NSColor.clearColor
    let clear_color: *mut AnyObject = msg_send![objc2::class!(NSColor), clearColor];
    let _: () = msg_send![ns_window, setBackgroundColor: clear_color];

    // Set the NSView's layer (CAMetalLayer) to non-opaque so the alpha channel
    // from the wgpu surface is composited with the desktop.
    let layer: *mut AnyObject = msg_send![ns_view, layer];
    if !layer.is_null() {
      let _: () = msg_send![layer, setOpaque: false];
    }

    // NSWindowCollectionBehavior: canJoinAllSpaces (1<<0) | fullScreenAuxiliary (1<<8)
    let behavior: u64 = (1 << 0) | (1 << 8);
    let _: () = msg_send![ns_window, setCollectionBehavior: behavior];

    // Set window level above everything (CGShieldingWindowLevel area, 1000+)
    // kCGScreenSaverWindowLevel = 1000 — above all normal windows
    let _: () = msg_send![ns_window, setLevel: 1000_i64];

    // Accept mouse events even when app is not frontmost
    let _: () = msg_send![ns_window, setAcceptsMouseMovedEvents: true];

    // Force to front — orderFrontRegardless bypasses activation checks
    let _: () = msg_send![ns_window, orderFrontRegardless];
    let _: () = msg_send![ns_window, makeKeyAndOrderFront: std::ptr::null::<AnyObject>()];
  }
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
