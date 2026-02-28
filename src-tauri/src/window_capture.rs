/// Win32 window detection for window capture mode.
///
/// Uses EnumWindows + DWM APIs to find the topmost visible window at a given
/// screen coordinate. Skips desktop, shell, cloaked, transparent, and zero-size
/// windows. Uses DwmGetWindowAttribute(DWMWA_EXTENDED_FRAME_BOUNDS) for tight
/// bounds (excludes invisible resize borders).

#[derive(Debug, Clone, serde::Serialize)]
pub struct WindowRect {
  pub left: i32,
  pub top: i32,
  pub right: i32,
  pub bottom: i32,
}

#[cfg(target_os = "windows")]
pub fn get_cursor_pos() -> (i32, i32) {
  use windows::Win32::Foundation::POINT;
  use windows::Win32::UI::WindowsAndMessaging::GetCursorPos;

  let mut pt = POINT::default();
  unsafe {
    if GetCursorPos(&mut pt).is_ok() {
      (pt.x, pt.y)
    } else {
      (0, 0)
    }
  }
}

#[cfg(not(target_os = "windows"))]
pub fn get_cursor_pos() -> (i32, i32) {
  (0, 0)
}

#[cfg(target_os = "windows")]
pub fn get_window_rect_at(x: i32, y: i32, exclude_hwnd: isize) -> Option<WindowRect> {
  use windows::Win32::Foundation::{BOOL, HWND, LPARAM, RECT};
  use windows::Win32::Graphics::Dwm::{
    DwmGetWindowAttribute, DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS,
  };
  use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetDesktopWindow, GetShellWindow, GetWindowLongW, GetWindowRect,
    IsWindowVisible, GWL_EXSTYLE, WS_EX_TRANSPARENT,
  };

  struct CallbackData {
    x: i32,
    y: i32,
    exclude: isize,
    desktop: HWND,
    shell: HWND,
    result: Option<WindowRect>,
  }

  fn is_cloaked(hwnd: HWND) -> bool {
    let mut cloaked: u32 = 0;
    let hr = unsafe {
      DwmGetWindowAttribute(
        hwnd,
        DWMWA_CLOAKED,
        &mut cloaked as *mut u32 as *mut _,
        std::mem::size_of::<u32>() as u32,
      )
    };
    hr.is_ok() && cloaked != 0
  }

  unsafe extern "system" fn callback(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let data = &mut *(lparam.0 as *mut CallbackData);

    let h = hwnd.0 as isize;
    if data.exclude != 0 && h == data.exclude {
      return BOOL(1);
    }
    if hwnd == data.desktop || hwnd == data.shell {
      return BOOL(1);
    }
    if !IsWindowVisible(hwnd).as_bool() {
      return BOOL(1);
    }
    if is_cloaked(hwnd) {
      return BOOL(1);
    }

    let exstyle = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32;
    if exstyle & WS_EX_TRANSPARENT.0 != 0 {
      return BOOL(1);
    }

    // Get tight window bounds (no invisible resize borders)
    let mut rect = RECT::default();
    let hr = DwmGetWindowAttribute(
      hwnd,
      DWMWA_EXTENDED_FRAME_BOUNDS,
      &mut rect as *mut RECT as *mut _,
      std::mem::size_of::<RECT>() as u32,
    );
    if hr.is_err() {
      let _ = GetWindowRect(hwnd, &mut rect);
    }

    if rect.right - rect.left < 1 || rect.bottom - rect.top < 1 {
      return BOOL(1);
    }

    if rect.left <= data.x && data.x < rect.right && rect.top <= data.y && data.y < rect.bottom {
      data.result = Some(WindowRect {
        left: rect.left,
        top: rect.top,
        right: rect.right,
        bottom: rect.bottom,
      });
      return BOOL(0); // Stop enumeration -- found topmost window
    }

    BOOL(1) // Continue
  }

  let desktop = unsafe { GetDesktopWindow() };
  let shell = unsafe { GetShellWindow() };

  let mut data = CallbackData {
    x,
    y,
    exclude: exclude_hwnd,
    desktop,
    shell,
    result: None,
  };

  unsafe {
    let _ = EnumWindows(Some(callback), LPARAM(&mut data as *mut CallbackData as isize));
  }

  data.result
}

#[cfg(not(target_os = "windows"))]
pub fn get_window_rect_at(_x: i32, _y: i32, _exclude_hwnd: isize) -> Option<WindowRect> {
  None
}
