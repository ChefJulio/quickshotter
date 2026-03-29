//! Window highlight border for window capture mode.
//!
//! Creates 4 thin colored windows forming a border around the detected window.
//! This avoids the complexity of per-pixel alpha rendering while being
//! visually clear and receiving no mouse input (WS_EX_TRANSPARENT).

#[cfg(target_os = "windows")]
use windows::Win32::Foundation::HWND;
#[cfg(target_os = "windows")]
use windows::Win32::UI::WindowsAndMessaging::*;

/// Four border windows forming a rectangle highlight.
#[cfg(target_os = "windows")]
pub struct HighlightBorder {
  top: HWND,
  bottom: HWND,
  left: HWND,
  right: HWND,
  visible: bool,
}

#[cfg(target_os = "windows")]
impl HighlightBorder {
  pub fn new() -> Self {
    let class_name = windows::core::w!("STATIC");
    let border_color = 0x00FFAA00u32; // COLORREF: BGR format for #00AAFF

    unsafe {
      // Create a brush for the border color
      let brush = windows::Win32::Graphics::Gdi::CreateSolidBrush(
        windows::Win32::Foundation::COLORREF(border_color)
      );

      let create = |name: &str| -> HWND {
        let hwnd = CreateWindowExW(
          WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
          class_name,
          windows::core::w!(""),
          WS_POPUP,
          0, 0, 0, 0,
          None, None, None, None,
        ).unwrap_or(HWND::default());

        // Set background color
        if !hwnd.is_invalid() {
          SetClassLongPtrW(hwnd, GCL_HBRBACKGROUND, brush.0 as isize);
        }
        hwnd
      };

      Self {
        top: create("top"),
        bottom: create("bottom"),
        left: create("left"),
        right: create("right"),
        visible: false,
      }
    }
  }

  /// Show the highlight border around a screen-space rectangle.
  pub fn show(&mut self, left: i32, top: i32, right: i32, bottom: i32) {
    let bw = 3; // border width

    unsafe {
      let flags = SWP_NOACTIVATE | SWP_SHOWWINDOW;

      // Top edge
      let _ = SetWindowPos(self.top, Some(HWND_TOPMOST),
        left - bw, top - bw, (right - left) + bw * 2, bw, flags);
      // Bottom edge
      let _ = SetWindowPos(self.bottom, Some(HWND_TOPMOST),
        left - bw, bottom, (right - left) + bw * 2, bw, flags);
      // Left edge
      let _ = SetWindowPos(self.left, Some(HWND_TOPMOST),
        left - bw, top, bw, bottom - top, flags);
      // Right edge
      let _ = SetWindowPos(self.right, Some(HWND_TOPMOST),
        right, top, bw, bottom - top, flags);
    }

    self.visible = true;
  }

  /// Hide the border.
  pub fn hide(&mut self) {
    if !self.visible { return; }
    unsafe {
      let _ = ShowWindow(self.top, SW_HIDE);
      let _ = ShowWindow(self.bottom, SW_HIDE);
      let _ = ShowWindow(self.left, SW_HIDE);
      let _ = ShowWindow(self.right, SW_HIDE);
    }
    self.visible = false;
  }
}

#[cfg(target_os = "windows")]
impl Drop for HighlightBorder {
  fn drop(&mut self) {
    unsafe {
      if !self.top.is_invalid() { let _ = DestroyWindow(self.top); }
      if !self.bottom.is_invalid() { let _ = DestroyWindow(self.bottom); }
      if !self.left.is_invalid() { let _ = DestroyWindow(self.left); }
      if !self.right.is_invalid() { let _ = DestroyWindow(self.right); }
    }
  }
}
