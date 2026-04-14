//! ScreenCaptureKit capture for macOS via the `screencapturekit` crate.
//!
//! Caches SCShareableContent and display info to avoid expensive
//! re-enumeration on every capture (~250ms → ~10ms).

use std::sync::Mutex;
use image::RgbaImage;
use screencapturekit::cg::CGRect;
use screencapturekit::cm::CMSampleBuffer;
use screencapturekit::cv::CVPixelBuffer;
use screencapturekit::shareable_content::SCShareableContent;
use screencapturekit::screenshot_manager::SCScreenshotManager;
use screencapturekit::stream::SCStream;
use screencapturekit::stream::configuration::SCStreamConfiguration;
use screencapturekit::stream::content_filter::SCContentFilter;
use screencapturekit::stream::output_type::SCStreamOutputType;
use screencapturekit::stream::output_trait::SCStreamOutputTrait;

use crate::error::AppError;
use crate::recording::pipeline::TimestampedFrame;

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
    pub scale: f64,
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
        scale: d.scale,
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

/// Capture all displays and stitch into a single image covering the virtual desktop.
/// Used for instant capture, OCR, and select_screen modes (NOT freeze mode —
/// freeze mode uses daemon-side xcap capture with zero IPC).
pub fn capture_all_displays() -> Result<(RgbaImage, i32, i32), AppError> {
    ensure_cache()?;
    let t0 = std::time::Instant::now();

    let guard = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    let displays = guard.as_ref().unwrap();

    if displays.is_empty() {
        return Err(AppError::Capture("No displays found".to_string()));
    }

    // Compute virtual desktop bounds in logical points
    let mut min_x = f64::MAX;
    let mut min_y = f64::MAX;
    let mut max_x = f64::MIN;
    let mut max_y = f64::MIN;

    for d in displays.iter() {
        min_x = min_x.min(d.frame.x);
        min_y = min_y.min(d.frame.y);
        max_x = max_x.max(d.frame.x + d.frame.width);
        max_y = max_y.max(d.frame.y + d.frame.height);
    }

    // Use backing scale for canvas (uniform scale across all displays).
    let canvas_scale = unsafe { qs_get_backing_scale_factor() };
    let canvas_w = ((max_x - min_x) * canvas_scale).ceil() as u32;
    let canvas_h = ((max_y - min_y) * canvas_scale).ceil() as u32;

    const MAX_PIXELS: u64 = 128_000_000;
    if (canvas_w as u64) * (canvas_h as u64) > MAX_PIXELS {
        return Err(AppError::Capture(format!(
            "Virtual desktop too large to capture ({}x{})", canvas_w, canvas_h
        )));
    }

    let mut canvas: RgbaImage = image::ImageBuffer::new(canvas_w, canvas_h);

    for d in displays.iter() {
        let dw = (d.frame.width * canvas_scale) as u32;
        let dh = (d.frame.height * canvas_scale) as u32;

        let config = SCStreamConfiguration::new()
            .with_width(dw)
            .with_height(dh)
            .with_shows_cursor(false);

        let img = SCScreenshotManager::capture_image(&d.filter, &config)
            .map_err(|e| AppError::Capture(format!("Display {} capture failed: {e}", d.display_id)))?;

        let rgba = img.rgba_data()
            .map_err(|e| AppError::Capture(format!("Pixel extraction failed: {e}")))?;
        let img_w = img.width() as u32;
        let img_h = img.height() as u32;

        if let Some(src) = RgbaImage::from_raw(img_w, img_h, rgba) {
            let ox = ((d.frame.x - min_x) * canvas_scale).round() as u32;
            let oy = ((d.frame.y - min_y) * canvas_scale).round() as u32;
            image::imageops::overlay(&mut canvas, &src, ox as i64, oy as i64);
            eprintln!("[sck] captured display {} ({}x{}) at ({},{})",
                d.display_id, img_w, img_h, ox, oy);
        }
    }

    drop(guard);
    eprintln!("[sck] all displays stitched ({}x{}) in {:?}", canvas_w, canvas_h, t0.elapsed());
    Ok((canvas, min_x as i32, min_y as i32))
}

/// Check if screen recording permission is granted.
pub fn check_permission() -> Result<(), AppError> {
    ensure_cache()?;
    Ok(())
}

// ── SCStream for recording ──────────────────────────────────────

/// Handle to a running SCStream capture.
pub struct StreamHandle {
    stream: SCStream,
    pub receiver: crossbeam_channel::Receiver<TimestampedFrame>,
}

// SCStream is Send+Sync per the crate; StreamHandle is used from the capture thread.
unsafe impl Send for StreamHandle {}

/// Output handler that converts CMSampleBuffer frames to TimestampedFrame
/// and sends them through a crossbeam channel.
struct FrameHandler {
    sender: crossbeam_channel::Sender<TimestampedFrame>,
    start_time: std::time::Instant,
}

impl SCStreamOutputTrait for FrameHandler {
    fn did_output_sample_buffer(&self, sample: CMSampleBuffer, of_type: SCStreamOutputType) {
        if of_type != SCStreamOutputType::Screen {
            return;
        }

        // Only process frames with actual content
        if let Some(status) = sample.frame_status() {
            if !status.has_content() {
                return;
            }
        }

        // Extract pixel buffer from the sample
        let pixel_buffer: CVPixelBuffer = match sample.image_buffer() {
            Some(pb) => pb,
            None => return,
        };

        let w = pixel_buffer.width() as u32;
        let h = pixel_buffer.height() as u32;
        if w == 0 || h == 0 {
            return;
        }

        // Lock pixel buffer for reading
        let guard = match pixel_buffer.lock_read_only() {
            Ok(g) => g,
            Err(_) => return,
        };

        let bytes_per_row = pixel_buffer.bytes_per_row();
        let expected_stride = w as usize * 4;

        // Copy BGRA data, converting to RGBA in-place.
        // Handle row padding (bytes_per_row may be > width*4).
        let mut rgba = Vec::with_capacity((w * h * 4) as usize);
        let src = guard.as_slice();
        for row in 0..h as usize {
            let row_start = row * bytes_per_row;
            let row_end = row_start + expected_stride;
            if row_end > src.len() {
                break;
            }
            let row_data = &src[row_start..row_end];
            // BGRA → RGBA: swap B and R channels
            for pixel in row_data.chunks_exact(4) {
                rgba.push(pixel[2]); // R (was B)
                rgba.push(pixel[1]); // G
                rgba.push(pixel[0]); // B (was R)
                rgba.push(pixel[3]); // A
            }
        }
        drop(guard);

        let image = match RgbaImage::from_raw(w, h, rgba) {
            Some(img) => img,
            None => return,
        };

        let pts_ms = self.start_time.elapsed().as_secs_f64() * 1000.0;

        // Non-blocking send — drop frame if encoder can't keep up
        let _ = self.sender.try_send(TimestampedFrame { image, pts_ms });
    }
}

/// Start an SCStream for recording a display (optionally cropped to a region).
///
/// - `display_id`: CGDirectDisplayID of the target display
/// - `region`: optional crop rect in display-local logical points (x, y, w, h)
/// - `fps`: target frame rate
/// - `output_width` / `output_height`: output resolution in physical pixels
pub fn start_stream(
    display_id: u32,
    region: Option<(f64, f64, f64, f64)>,
    fps: u32,
    output_width: u32,
    output_height: u32,
) -> Result<StreamHandle, AppError> {
    ensure_cache()?;

    let guard = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    let displays = guard.as_ref().unwrap();

    let target = displays.iter().find(|d| d.display_id == display_id)
        .ok_or_else(|| AppError::Recording(format!(
            "Display {} not found in SCK cache", display_id
        )))?;

    // Configure stream
    let mut config = SCStreamConfiguration::new()
        .with_width(output_width)
        .with_height(output_height)
        .with_pixel_format(screencapturekit::stream::configuration::pixel_format::PixelFormat::BGRA)
        .with_shows_cursor(false)
        .with_fps(fps);

    if let Some((x, y, w, h)) = region {
        config = config.with_source_rect(CGRect::new(x, y, w, h));
    }

    // Create stream while we still hold the cache lock (need the filter ref)
    let mut stream = SCStream::new(&target.filter, &config);

    // Drop the cache lock before starting capture
    drop(guard);

    // Channel for frame delivery (4-frame buffer matches existing pipeline)
    let (sender, receiver) = crossbeam_channel::bounded(4);

    let handler = FrameHandler {
        sender,
        start_time: std::time::Instant::now(),
    };

    stream.add_output_handler(handler, SCStreamOutputType::Screen);

    stream.start_capture()
        .map_err(|e| AppError::Recording(format!("SCStream start failed: {e}")))?;

    eprintln!("[sck] stream started: display={} {}x{} @ {}fps region={:?}",
        display_id, output_width, output_height, fps, region);

    Ok(StreamHandle { stream, receiver })
}

/// Stop a running SCStream.
pub fn stop_stream(handle: StreamHandle) {
    if let Err(e) = handle.stream.stop_capture() {
        eprintln!("[sck] stream stop error: {e}");
    }
    eprintln!("[sck] stream stopped");
}
