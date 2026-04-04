//! ScreenCaptureKit capture for macOS via the `screencapturekit` crate.
//!
//! Replaces CGWindowListCreateImage with SCScreenshotManager for faster,
//! GPU-accelerated capture with proper permission handling.

use image::RgbaImage;
use screencapturekit::cg::CGRect;
use screencapturekit::shareable_content::SCShareableContent;
use screencapturekit::screenshot_manager::SCScreenshotManager;
use screencapturekit::stream::configuration::SCStreamConfiguration;
use screencapturekit::stream::content_filter::SCContentFilter;

use crate::error::AppError;

extern "C" {
    fn qs_get_backing_scale_factor() -> f64;
}

fn get_backing_scale_factor() -> f64 {
    let scale = unsafe { qs_get_backing_scale_factor() };
    eprintln!("[retina] NSScreen.backingScaleFactor = {scale}");
    scale
}

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
    let content = SCShareableContent::get()
        .map_err(|e| AppError::Capture(format!("ScreenCaptureKit: {e}")))?;

    let displays = content.displays();
    Ok(displays.iter().map(|d| {
        let frame = d.frame();
        DisplayInfo {
            display_id: d.display_id(),
            x: frame.x as i32,
            y: frame.y as i32,
            width: frame.width as u32,
            height: frame.height as u32,
        }
    }).collect())
}

/// Capture a screen region. Coordinates are logical points.
/// Returns a Retina-resolution RGBA image.
pub fn capture_region(x: f64, y: f64, w: f64, h: f64) -> Result<RgbaImage, AppError> {
    let content = SCShareableContent::get()
        .map_err(|e| AppError::Capture(format!("ScreenCaptureKit: {e}")))?;

    let displays = content.displays();
    if displays.is_empty() {
        return Err(AppError::Capture("No displays found".to_string()));
    }

    // Find the display that contains the region center
    let cx = x + w / 2.0;
    let cy = y + h / 2.0;
    let target = displays.iter().find(|d| {
        let f = d.frame();
        cx >= f.x && cx < f.x + f.width && cy >= f.y && cy < f.y + f.height
    }).unwrap_or(&displays[0]);

    let display_frame = target.frame();

    // Build content filter for target display
    let filter = SCContentFilter::create()
        .with_display(target)
        .with_excluding_windows(&[])
        .build();

    // Get Retina scale via NSScreen.backingScaleFactor.
    // CGDisplayPixelsWide returns logical on some macOS versions, so use NSScreen.
    let scale = get_backing_scale_factor();

    // Source rect in display-local logical coordinates
    let source_rect = CGRect::new(
        x - display_frame.x,
        y - display_frame.y,
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

    let image = SCScreenshotManager::capture_image(&filter, &config)
        .map_err(|e| AppError::Capture(format!("Screenshot failed: {e}")))?;

    let rgba = image.rgba_data()
        .map_err(|e| AppError::Capture(format!("Pixel extraction failed: {e}")))?;

    let img_w = image.width() as u32;
    let img_h = image.height() as u32;

    RgbaImage::from_raw(img_w, img_h, rgba)
        .ok_or_else(|| AppError::Capture("Failed to create image from SCK data".to_string()))
}

/// Check if screen recording permission is granted.
pub fn check_permission() -> Result<(), AppError> {
    SCShareableContent::get()
        .map_err(|e| AppError::Capture(format!("Screen recording permission denied: {e}")))?;
    Ok(())
}
