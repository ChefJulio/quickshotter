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

/// Convert RGBA image to RGB bytes without cloning the source image.
/// Strips the alpha channel, saving ~33% peak memory vs DynamicImage clone.
fn rgba_to_rgb(img: &RgbaImage) -> image::RgbImage {
  let (w, h) = (img.width(), img.height());
  let rgb_bytes: Vec<u8> = img
    .as_raw()
    .chunks_exact(4)
    .flat_map(|px| [px[0], px[1], px[2]])
    .collect();
  image::RgbImage::from_raw(w, h, rgb_bytes).expect("RGB buffer size mismatch")
}

/// Encode an image as JPEG base64 for sending to the overlay webview.
/// Uses JPEG instead of PNG -- ~10x faster encoding, only used for preview.
pub fn image_to_base64(img: &RgbaImage) -> Result<String, AppError> {
  let rgb = rgba_to_rgb(img);
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

  // Collision avoidance: append _2, _3, etc. if file exists
  let mut counter = 2u32;
  while filepath.exists() {
    filepath = folder.join(format!("{}_{}.{}", base_name, counter, ext));
    counter += 1;
  }

  match ext.as_str() {
    "jpg" | "jpeg" => {
      let rgb = rgba_to_rgb(img);
      let mut buf = Cursor::new(Vec::new());
      let mut encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, 85);
      encoder
        .encode(&rgb, rgb.width(), rgb.height(), image::ExtendedColorType::Rgb8)
        .map_err(|e| AppError::Capture(e.to_string()))?;
      std::fs::write(&filepath, buf.into_inner())?;
    }
    "webp" => {
      let rgba = image::DynamicImage::ImageRgba8(img.clone());
      let mut buf = Cursor::new(Vec::new());
      rgba
        .write_to(&mut buf, image::ImageFormat::WebP)
        .map_err(|e| AppError::Capture(e.to_string()))?;
      std::fs::write(&filepath, buf.into_inner())?;
    }
    _ => {
      // PNG (default)
      img.save(&filepath).map_err(|e| AppError::Capture(e.to_string()))?;
    }
  }

  Ok(Some(filepath))
}
