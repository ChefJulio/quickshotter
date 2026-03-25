use arboard::Clipboard;
use base64::Engine;
use image::{GenericImage, ImageBuffer, ImageEncoder, RgbaImage};
use std::io::Cursor;
use std::path::PathBuf;
use xcap::Monitor;

use crate::config::AppConfig;
use crate::error::AppError;

/// Virtual desktop bounds returned alongside the stitched screenshot.
#[allow(dead_code)]
pub struct ScreenCapture {
  pub image: RgbaImage,
  pub origin_x: i32,
  pub origin_y: i32,
  pub width: u32,
  pub height: u32,
}

/// Virtual desktop bounds (no image capture -- fast).
pub struct DesktopBounds {
  pub x: i32,
  pub y: i32,
  pub width: u32,
  pub height: u32,
}

/// Get virtual desktop bounds without capturing any images.
pub fn get_desktop_bounds() -> Result<DesktopBounds, AppError> {
  let monitors = Monitor::all().map_err(|e| AppError::Capture(e.to_string()))?;
  if monitors.is_empty() {
    return Err(AppError::Capture("No monitors found".to_string()));
  }
  if monitors.len() == 1 {
    let m = &monitors[0];
    return Ok(DesktopBounds {
      x: m.x().map_err(|e| AppError::Capture(e.to_string()))?,
      y: m.y().map_err(|e| AppError::Capture(e.to_string()))?,
      width: m.width().map_err(|e| AppError::Capture(e.to_string()))?,
      height: m.height().map_err(|e| AppError::Capture(e.to_string()))?,
    });
  }
  let mut min_x = i32::MAX;
  let mut min_y = i32::MAX;
  let mut max_x = i32::MIN;
  let mut max_y = i32::MIN;
  for m in &monitors {
    let x = m.x().map_err(|e| AppError::Capture(e.to_string()))?;
    let y = m.y().map_err(|e| AppError::Capture(e.to_string()))?;
    let w = m.width().map_err(|e| AppError::Capture(e.to_string()))? as i32;
    let h = m.height().map_err(|e| AppError::Capture(e.to_string()))? as i32;
    min_x = min_x.min(x);
    min_y = min_y.min(y);
    max_x = max_x.max(x + w);
    max_y = max_y.max(y + h);
  }
  Ok(DesktopBounds {
    x: min_x,
    y: min_y,
    width: (max_x - min_x) as u32,
    height: (max_y - min_y) as u32,
  })
}

/// Capture all monitors and stitch into a single image covering the virtual desktop.
pub fn capture_all_monitors() -> Result<ScreenCapture, AppError> {
  let monitors = Monitor::all().map_err(|e| AppError::Capture(e.to_string()))?;
  if monitors.is_empty() {
    return Err(AppError::Capture("No monitors found".to_string()));
  }

  // Single monitor fast path -- skip stitching entirely
  if monitors.len() == 1 {
    let m = &monitors[0];
    let img = m.capture_image().map_err(|e| AppError::Capture(e.to_string()))?;
    let x = m.x().map_err(|e| AppError::Capture(e.to_string()))?;
    let y = m.y().map_err(|e| AppError::Capture(e.to_string()))?;
    return Ok(ScreenCapture {
      width: img.width(),
      height: img.height(),
      origin_x: x,
      origin_y: y,
      image: img,
    });
  }

  // Multi-monitor: capture each monitor and compute DPI-aware stitching.
  // xcap may return logical or physical coordinates depending on platform;
  // we derive the effective scale per monitor by comparing capture image
  // dimensions to reported dimensions, then stitch at max_scale so the
  // canvas is at uniform physical resolution.
  struct MonCapture {
    img: RgbaImage,
    reported_x: i32,
    reported_y: i32,
    reported_w: u32,
    reported_h: u32,
  }

  let mut captures = Vec::new();
  let mut min_x = i32::MAX;
  let mut min_y = i32::MAX;
  let mut max_x = i32::MIN;
  let mut max_y = i32::MIN;
  let mut max_scale: f64 = 1.0;

  for m in &monitors {
    let img = m.capture_image().map_err(|e| AppError::Capture(e.to_string()))?;
    let rx = m.x().map_err(|e| AppError::Capture(e.to_string()))?;
    let ry = m.y().map_err(|e| AppError::Capture(e.to_string()))?;
    let rw = m.width().map_err(|e| AppError::Capture(e.to_string()))?;
    let rh = m.height().map_err(|e| AppError::Capture(e.to_string()))?;

    let scale = img.width() as f64 / rw.max(1) as f64;
    max_scale = max_scale.max(scale);

    min_x = min_x.min(rx);
    min_y = min_y.min(ry);
    max_x = max_x.max(rx + rw as i32);
    max_y = max_y.max(ry + rh as i32);

    captures.push(MonCapture { img, reported_x: rx, reported_y: ry, reported_w: rw, reported_h: rh });
  }

  // Canvas at max_scale: reported dimensions * max_scale = physical pixels.
  // On uniform-DPI setups max_scale is 1.0 (if xcap returns physical) or
  // uniform (e.g. 2.0 on all-Retina), so this is equivalent to current code.
  let total_w = ((max_x - min_x) as f64 * max_scale).ceil() as u32;
  let total_h = ((max_y - min_y) as f64 * max_scale).ceil() as u32;

  // Guard against unreasonable allocations from multi-monitor setups
  const MAX_PIXELS: u64 = 64_000_000; // ~256MB for RGBA
  if (total_w as u64) * (total_h as u64) > MAX_PIXELS {
    return Err(AppError::Capture(format!(
      "Virtual desktop too large to capture ({}x{})",
      total_w, total_h
    )));
  }

  let mut canvas: RgbaImage = ImageBuffer::new(total_w, total_h);

  for c in &captures {
    let offset_x = ((c.reported_x - min_x) as f64 * max_scale).round() as u32;
    let offset_y = ((c.reported_y - min_y) as f64 * max_scale).round() as u32;
    let target_w = (c.reported_w as f64 * max_scale).round() as u32;
    let target_h = (c.reported_h as f64 * max_scale).round() as u32;

    if c.img.width() == target_w && c.img.height() == target_h {
      // Same scale -- place directly (common: uniform DPI or xcap returns physical)
      canvas.copy_from(&c.img, offset_x, offset_y)
        .map_err(|e| AppError::Capture(e.to_string()))?;
    } else {
      // Mixed DPI -- resize lower-DPI capture up to max_scale
      let resized = image::imageops::resize(
        &c.img, target_w, target_h, image::imageops::FilterType::Triangle,
      );
      canvas.copy_from(&resized, offset_x, offset_y)
        .map_err(|e| AppError::Capture(e.to_string()))?;
    }
  }

  Ok(ScreenCapture {
    image: canvas,
    origin_x: min_x,
    origin_y: min_y,
    width: total_w,
    height: total_h,
  })
}

/// Capture a single monitor by index.
pub fn capture_monitor(index: usize) -> Result<ScreenCapture, AppError> {
  let monitors = Monitor::all().map_err(|e| AppError::Capture(e.to_string()))?;
  let m = monitors.get(index).ok_or_else(|| AppError::Capture(format!("Monitor index {} out of range", index)))?;
  let img = m.capture_image().map_err(|e| AppError::Capture(e.to_string()))?;
  let x = m.x().map_err(|e| AppError::Capture(e.to_string()))?;
  let y = m.y().map_err(|e| AppError::Capture(e.to_string()))?;
  Ok(ScreenCapture {
    width: img.width(),
    height: img.height(),
    origin_x: x,
    origin_y: y,
    image: img,
  })
}

/// Find which monitor the cursor is on and capture it.
pub fn capture_monitor_at_cursor() -> Result<ScreenCapture, AppError> {
  let (cx, cy) = get_cursor_position()?;
  let monitors = Monitor::all().map_err(|e| AppError::Capture(e.to_string()))?;
  // Find monitor containing cursor
  let index = monitors.iter().position(|m| {
    let Ok(mx) = m.x() else { return false };
    let Ok(my) = m.y() else { return false };
    let Ok(mw) = m.width() else { return false };
    let Ok(mh) = m.height() else { return false };
    cx >= mx && cx < mx + mw as i32 && cy >= my && cy < my + mh as i32
  }).unwrap_or(0); // fall back to primary if cursor not found on any
  capture_monitor(index)
}

/// Return info about all monitors (bounds + names).
pub fn get_monitor_info() -> Result<Vec<MonitorInfo>, AppError> {
  let monitors = Monitor::all().map_err(|e| AppError::Capture(e.to_string()))?;
  let mut result = Vec::new();
  for (i, m) in monitors.iter().enumerate() {
    let x = m.x().map_err(|e| AppError::Capture(e.to_string()))?;
    let y = m.y().map_err(|e| AppError::Capture(e.to_string()))?;
    let w = m.width().map_err(|e| AppError::Capture(e.to_string()))?;
    let h = m.height().map_err(|e| AppError::Capture(e.to_string()))?;
    result.push(MonitorInfo { index: i, x, y, width: w, height: h });
  }
  Ok(result)
}

#[derive(Clone, serde::Serialize)]
pub struct MonitorInfo {
  pub index: usize,
  pub x: i32,
  pub y: i32,
  pub width: u32,
  pub height: u32,
}

/// Get cursor position (platform-specific).
#[cfg(target_os = "windows")]
fn get_cursor_position() -> Result<(i32, i32), AppError> {
  use windows::Win32::UI::WindowsAndMessaging::GetCursorPos;
  use windows::Win32::Foundation::POINT;
  let mut pt = POINT { x: 0, y: 0 };
  unsafe {
    GetCursorPos(&mut pt).map_err(|e| AppError::Capture(format!("GetCursorPos: {e}")))?;
  }
  Ok((pt.x, pt.y))
}

#[cfg(target_os = "macos")]
fn get_cursor_position() -> Result<(i32, i32), AppError> {
  // Use CoreGraphics FFI directly to avoid adding core-graphics crate dependency.
  // NSEvent.mouseLocation returns cursor position in screen coordinates.
  #[repr(C)]
  #[derive(Copy, Clone)]
  struct CGPoint { x: f64, y: f64 }

  #[link(name = "CoreGraphics", kind = "framework")]
  extern "C" {
    fn CGEventCreate(source: *const std::ffi::c_void) -> *const std::ffi::c_void;
    fn CGEventGetLocation(event: *const std::ffi::c_void) -> CGPoint;
    fn CFRelease(cf: *const std::ffi::c_void);
  }

  unsafe {
    let event = CGEventCreate(std::ptr::null());
    if event.is_null() {
      return Err(AppError::Capture("CGEventCreate returned null".to_string()));
    }
    let loc = CGEventGetLocation(event);
    CFRelease(event);
    Ok((loc.x as i32, loc.y as i32))
  }
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
fn get_cursor_position() -> Result<(i32, i32), AppError> {
  // Fallback: return origin, will default to primary monitor
  Ok((0, 0))
}

/// Convert RGBA image to RGB bytes without cloning the source image.
/// Strips the alpha channel, saving ~33% peak memory vs DynamicImage clone.
fn rgba_to_rgb(img: &RgbaImage) -> Result<image::RgbImage, AppError> {
  let (w, h) = (img.width(), img.height());
  let rgb_bytes: Vec<u8> = img
    .as_raw()
    .chunks_exact(4)
    .flat_map(|px| [px[0], px[1], px[2]])
    .collect();
  image::RgbImage::from_raw(w, h, rgb_bytes)
    .ok_or_else(|| AppError::Capture("RGB buffer size mismatch".to_string()))
}

/// Encode an image as JPEG base64 for sending to the overlay webview.
/// Uses JPEG instead of PNG -- ~10x faster encoding, only used for preview.
pub fn image_to_base64(img: &RgbaImage) -> Result<String, AppError> {
  let rgb = rgba_to_rgb(img)?;
  let mut buf = Cursor::new(Vec::new());
  let mut encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, 80);
  encoder
    .encode(&rgb, rgb.width(), rgb.height(), image::ExtendedColorType::Rgb8)
    .map_err(|e| AppError::Capture(e.to_string()))?;
  Ok(base64::engine::general_purpose::STANDARD.encode(buf.into_inner()))
}

/// Encode an image as lossless PNG base64 for the annotation editor.
pub fn image_to_base64_png(img: &RgbaImage) -> Result<String, AppError> {
  let mut buf = Cursor::new(Vec::new());
  let encoder = image::codecs::png::PngEncoder::new(&mut buf);
  encoder
    .write_image(img.as_raw(), img.width(), img.height(), image::ExtendedColorType::Rgba8)
    .map_err(|e: image::ImageError| AppError::Capture(e.to_string()))?;
  Ok(base64::engine::general_purpose::STANDARD.encode(buf.into_inner()))
}

/// Check if the app has screen recording permission on macOS.
/// Uses CGPreflightScreenCaptureAccess (macOS 10.15+) to check without prompting.
#[cfg(target_os = "macos")]
pub fn has_screen_recording_permission() -> bool {
  extern "C" {
    fn CGPreflightScreenCaptureAccess() -> bool;
  }
  unsafe { CGPreflightScreenCaptureAccess() }
}

/// Request screen recording permission on macOS.
/// Triggers the system permission dialog on first call.
#[cfg(target_os = "macos")]
pub fn request_screen_recording_permission() {
  extern "C" {
    fn CGRequestScreenCaptureAccess() -> bool;
  }
  unsafe { CGRequestScreenCaptureAccess(); }
}

/// Check if a captured image is likely blank (all black pixels).
/// On macOS, this typically means screen recording permission was denied.
#[cfg(target_os = "macos")]
pub fn is_likely_blank(img: &RgbaImage) -> bool {
  if img.width() == 0 || img.height() == 0 {
    return true;
  }
  // Sample every ~997th pixel (prime stride avoids repeating patterns)
  img
    .as_raw()
    .chunks_exact(4)
    .step_by(997)
    .all(|px| px[0] == 0 && px[1] == 0 && px[2] == 0)
}

/// Copy an RGBA image to the system clipboard.
pub fn copy_to_clipboard(img: &RgbaImage) -> Result<(), AppError> {
  let mut clipboard = Clipboard::new().map_err(|e| AppError::Clipboard(e.to_string()))?;
  let img_data = arboard::ImageData {
    width: img.width() as usize,
    height: img.height() as usize,
    bytes: std::borrow::Cow::Borrowed(img.as_raw()),
  };
  clipboard
    .set_image(img_data)
    .map_err(|e| AppError::Clipboard(e.to_string()))?;
  Ok(())
}

/// Copy a file to the system clipboard as a file drop (like Ctrl+C in Explorer).
/// This lets users paste the file into Discord, Slack, email, file managers, etc.
#[cfg(target_os = "windows")]
pub fn copy_file_to_clipboard(path: &std::path::Path) -> Result<(), AppError> {
  use std::os::windows::ffi::OsStrExt;
  use windows::Win32::System::DataExchange::{
    OpenClipboard, CloseClipboard, EmptyClipboard, SetClipboardData,
  };
  use windows::Win32::System::Memory::{
    GlobalAlloc, GlobalLock, GlobalUnlock, GLOBAL_ALLOC_FLAGS,
  };

  // DROPFILES struct layout (20 bytes):
  //   u32 pFiles  - offset to file list
  //   i32 pt.x    - drop point x (unused, 0)
  //   i32 pt.y    - drop point y (unused, 0)
  //   i32 fNC     - non-client area flag (0)
  //   i32 fWide   - 1 = Unicode paths
  #[repr(C)]
  struct DropFiles {
    p_files: u32,
    pt_x: i32,
    pt_y: i32,
    f_nc: i32,
    f_wide: i32,
  }

  const CF_HDROP: u32 = 15;
  const GMEM_MOVEABLE: u32 = 0x0002;
  const GMEM_ZEROINIT: u32 = 0x0040;

  let wide_path: Vec<u16> = path.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
  let header_size = std::mem::size_of::<DropFiles>();
  // file path + extra null terminator for double-null termination
  let total_size = header_size + (wide_path.len() + 1) * 2;

  unsafe {
    let hmem = GlobalAlloc(GLOBAL_ALLOC_FLAGS(GMEM_MOVEABLE | GMEM_ZEROINIT), total_size)
      .map_err(|e| AppError::Clipboard(format!("GlobalAlloc failed: {e}")))?;
    let ptr = GlobalLock(hmem) as *mut u8;
    if ptr.is_null() {
      return Err(AppError::Clipboard("GlobalLock returned null".to_string()));
    }

    // Fill DROPFILES header
    let header = ptr as *mut DropFiles;
    (*header).p_files = header_size as u32;
    (*header).f_wide = 1;

    // Copy wide path after header
    let file_list = ptr.add(header_size) as *mut u16;
    std::ptr::copy_nonoverlapping(wide_path.as_ptr(), file_list, wide_path.len());
    // Double-null termination is already zeroed by GMEM_ZEROINIT

    let _ = GlobalUnlock(hmem);

    OpenClipboard(None)
      .map_err(|e| AppError::Clipboard(format!("OpenClipboard failed: {e}")))?;
    EmptyClipboard()
      .map_err(|e| {
        let _ = CloseClipboard();
        AppError::Clipboard(format!("EmptyClipboard failed: {e}"))
      })?;

    use windows::Win32::Foundation::HANDLE;
    SetClipboardData(CF_HDROP, Some(HANDLE(hmem.0 as *mut _)))
      .map_err(|e| {
        let _ = CloseClipboard();
        AppError::Clipboard(format!("SetClipboardData failed: {e}"))
      })?;
    let _ = CloseClipboard();
  }

  Ok(())
}

#[cfg(target_os = "macos")]
pub fn copy_file_to_clipboard(path: &std::path::Path) -> Result<(), AppError> {
  // Use `pbcopy`-style approach: run a small AppleScript/osascript to set the
  // file on the pasteboard. This is simpler than linking NSPasteboard via ObjC FFI
  // and handles all the pasteboard types correctly for Finder paste.
  let path_str = path.to_string_lossy();
  let script = format!(
    r#"set the clipboard to (POSIX file "{}")"#,
    path_str.replace('\\', "\\\\").replace('"', "\\\"")
  );
  let output = std::process::Command::new("osascript")
    .args(["-e", &script])
    .output()
    .map_err(|e| AppError::Clipboard(format!("osascript failed: {e}")))?;
  if !output.status.success() {
    let stderr = String::from_utf8_lossy(&output.stderr);
    return Err(AppError::Clipboard(format!("osascript error: {stderr}")));
  }
  Ok(())
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
pub fn copy_file_to_clipboard(_path: &std::path::Path) -> Result<(), AppError> {
  // Linux: not yet implemented; silently succeed
  Ok(())
}

/// Strip path-significant and Windows-reserved characters from filename parts.
fn sanitize_filename_part(s: &str) -> String {
  s.replace(['/', '\\', ':', '<', '>', '|', '"', '?', '*', '\0'], "_")
    .replace("..", "_")
}

/// Save an image to disk based on config. Returns the filepath if saved.
pub fn save_to_disk(img: &RgbaImage, config: &AppConfig) -> Result<Option<PathBuf>, AppError> {
  if !config.save_to_disk {
    return Ok(None);
  }

  let folder = PathBuf::from(&config.save_folder);
  std::fs::create_dir_all(&folder)?;

  let ext = &config.format;
  // Sanitize prefix and suffix to prevent path traversal
  let safe_prefix = sanitize_filename_part(&config.filename_prefix);
  let safe_suffix = sanitize_filename_part(&config.filename_suffix);
  // Validate chrono format string; fall back to default if invalid
  let timestamp = {
    let has_error = chrono::format::strftime::StrftimeItems::new(&safe_suffix)
      .any(|item| matches!(item, chrono::format::Item::Error));
    if has_error {
      chrono::Local::now().format("%Y-%m-%d_%H-%M-%S").to_string()
    } else {
      chrono::Local::now().format(&safe_suffix).to_string()
    }
  };
  let base_name = format!("{}_{}", safe_prefix, timestamp);
  // Final safety net: strip any remaining directory components
  let base_name = std::path::Path::new(&base_name)
    .file_name()
    .map(|n| n.to_string_lossy().to_string())
    .unwrap_or_else(|| format!("screenshot_{}", chrono::Local::now().format("%Y-%m-%d_%H-%M-%S")));
  let mut filepath = folder.join(format!("{}.{}", base_name, ext));

  // Collision avoidance: append _2, _3, etc. if file exists (bounded)
  let mut counter = 2u32;
  while filepath.exists() && counter < 10_000 {
    filepath = folder.join(format!("{}_{}.{}", base_name, counter, ext));
    counter += 1;
  }

  match ext.as_str() {
    "jpg" | "jpeg" => {
      // Stream JPEG directly to file via BufWriter -- avoids buffering the
      // entire encoded image in memory before writing.
      let rgb = rgba_to_rgb(img)?;
      let file = std::fs::File::create(&filepath)?;
      let mut writer = std::io::BufWriter::new(file);
      let mut encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut writer, 85);
      encoder
        .encode(&rgb, rgb.width(), rgb.height(), image::ExtendedColorType::Rgb8)
        .map_err(|e| AppError::Capture(e.to_string()))?;
    }
    "webp" => {
      // Encode directly from raw RGBA bytes -- avoids cloning the entire
      // image into a DynamicImage wrapper and the intermediate Vec<u8> buffer.
      let file = std::fs::File::create(&filepath)?;
      let mut writer = std::io::BufWriter::new(file);
      let encoder = image::codecs::webp::WebPEncoder::new_lossless(&mut writer);
      encoder
        .write_image(img.as_raw(), img.width(), img.height(), image::ExtendedColorType::Rgba8)
        .map_err(|e| AppError::Capture(e.to_string()))?;
    }
    _ => {
      // PNG (default)
      img.save(&filepath).map_err(|e| AppError::Capture(e.to_string()))?;
    }
  }

  Ok(Some(filepath))
}
