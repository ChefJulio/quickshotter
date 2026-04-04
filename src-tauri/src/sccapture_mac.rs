//! ScreenCaptureKit capture for macOS via the `screencapturekit` crate.
//!
//! Caches SCShareableContent and display info to avoid expensive
//! re-enumeration on every capture (~250ms → ~10ms).

use std::sync::Mutex;
use image::RgbaImage;
use screencapturekit::cg::CGRect;
use screencapturekit::shareable_content::{SCShareableContent, display::SCDisplay};
use screencapturekit::screenshot_manager::SCScreenshotManager;
use screencapturekit::stream::configuration::SCStreamConfiguration;
use screencapturekit::stream::content_filter::SCContentFilter;

use crate::error::AppError;

extern "C" {
    fn qs_get_backing_scale_factor() -> f64;
}

// ── Cached state ──────────────────────────────────────────────────

struct CachedDisplay {
    display_id: u32,
    frame: CGRect,
    scale: f64,
    filter: SCContentFilter,
}

static CACHE: Mutex<Option<Vec<CachedDisplay>>> = Mutex::new(None);

/// Initialize or refresh the display cache.
fn ensure_cache() -> Result<(), AppError> {
    let mut guard = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if guard.is_some() { return Ok(()); }

    let t0 = std::time::Instant::now();
    let content = SCShareableContent::get()
        .map_err(|e| AppError::Capture(format!("ScreenCaptureKit: {e}")))?;

    let scale = unsafe { qs_get_backing_scale_factor() };
    eprintln!("[sck] backingScaleFactor = {scale}");

    let displays: Vec<CachedDisplay> = content.displays().iter().map(|d| {
        let filter = SCContentFilter::create()
            .with_display(d)
            .with_excluding_windows(&[])
            .build();
        CachedDisplay {
            display_id: d.display_id(),
            frame: d.frame(),
            scale,
            filter,
        }
    }).collect();

    eprintln!("[sck] cached {} displays in {:?}", displays.len(), t0.elapsed());
    *guard = Some(displays);
    Ok(())
}

/// Invalidate the cache (call if displays change).
#[allow(dead_code)]
pub fn invalidate_cache() {
    *CACHE.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

// ── Public API ────────────────────────────────────────────────────

/// Display info extracted from ScreenCaptureKit.
pub struct DisplayInfo {
    pub display_id: u32,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

/// Get available displays. Also triggers permission prompt if needed.
pub fn get_displays() -> Result<Vec<DisplayInfo>, AppError> {
    ensure_cache()?;
    let guard = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    let displays = guard.as_ref().unwrap();
    Ok(displays.iter().map(|d| DisplayInfo {
        display_id: d.display_id,
        x: d.frame.x as i32,
        y: d.frame.y as i32,
        width: d.frame.width as u32,
        height: d.frame.height as u32,
    }).collect())
}

/// Capture a screen region. Coordinates are logical points.
/// Returns a Retina-resolution RGBA image.
pub fn capture_region(x: f64, y: f64, w: f64, h: f64) -> Result<RgbaImage, AppError> {
    ensure_cache()?;
    let t0 = std::time::Instant::now();

    let guard = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    let displays = guard.as_ref().unwrap();

    if displays.is_empty() {
        return Err(AppError::Capture("No displays found".to_string()));
    }

    // Find the display that contains the region center
    let cx = x + w / 2.0;
    let cy = y + h / 2.0;
    let target = displays.iter().find(|d| {
        cx >= d.frame.x && cx < d.frame.x + d.frame.width
            && cy >= d.frame.y && cy < d.frame.y + d.frame.height
    }).unwrap_or(&displays[0]);

    let scale = target.scale;

    // Source rect in display-local logical coordinates
    let source_rect = CGRect::new(
        x - target.frame.x,
        y - target.frame.y,
        w, h,
    );

    // Output dimensions in physical pixels
    let pixel_w = (w * scale) as u32;
    let pixel_h = (h * scale) as u32;

    let config = SCStreamConfiguration::new()
        .with_width(pixel_w)
        .with_height(pixel_h)
        .with_source_rect(source_rect)
        .with_shows_cursor(false);

    // Clone the filter ref — SCScreenshotManager needs it by reference
    // but we hold the mutex. Take what we need and drop the lock.
    let filter = &target.filter;

    let image = SCScreenshotManager::capture_image(filter, &config)
        .map_err(|e| AppError::Capture(format!("Screenshot failed: {e}")))?;

    // Drop the lock before pixel extraction
    drop(guard);

    let rgba = image.rgba_data()
        .map_err(|e| AppError::Capture(format!("Pixel extraction failed: {e}")))?;

    let img_w = image.width() as u32;
    let img_h = image.height() as u32;

    eprintln!("[sck] capture_region ({}x{} → {}x{}) in {:?}", w, h, img_w, img_h, t0.elapsed());

    RgbaImage::from_raw(img_w, img_h, rgba)
        .ok_or_else(|| AppError::Capture("Failed to create image from SCK data".to_string()))
}

/// Check if screen recording permission is granted.
pub fn check_permission() -> Result<(), AppError> {
    ensure_cache()?;
    Ok(())
}
