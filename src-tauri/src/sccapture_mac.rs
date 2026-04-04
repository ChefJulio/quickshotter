//! ScreenCaptureKit FFI bindings for macOS.
//!
//! Wraps the C API exposed by sccapture.m. Used on macOS to replace
//! CGWindowListCreateImage with SCScreenshotManager for faster, more
//! reliable screen capture with proper permission handling.

use image::RgbaImage;
use crate::error::AppError;

#[repr(C)]
#[derive(Debug)]
pub struct SCDisplayInfo {
    pub display_id: u32,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

extern "C" {
    fn sccapture_get_displays(
        out_displays: *mut *mut SCDisplayInfo,
        out_count: *mut i32,
        error_buf: *mut u8,
        error_buf_len: i32,
    ) -> i32;

    fn sccapture_capture_region(
        x: f64, y: f64, w: f64, h: f64,
        out_rgba: *mut *mut u8,
        out_width: *mut u32,
        out_height: *mut u32,
        error_buf: *mut u8,
        error_buf_len: i32,
    ) -> i32;

    fn sccapture_capture_display(
        display_id: u32,
        out_rgba: *mut *mut u8,
        out_width: *mut u32,
        out_height: *mut u32,
        error_buf: *mut u8,
        error_buf_len: i32,
    ) -> i32;

    fn sccapture_check_permission(
        error_buf: *mut u8,
        error_buf_len: i32,
    ) -> i32;

    fn sccapture_free_pixels(pixels: *mut u8);
}

fn get_error(buf: &[u8]) -> String {
    let len = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    String::from_utf8_lossy(&buf[..len]).to_string()
}

/// Get available displays. Also triggers permission prompt if needed.
pub fn get_displays() -> Result<Vec<SCDisplayInfo>, AppError> {
    let mut displays_ptr: *mut SCDisplayInfo = std::ptr::null_mut();
    let mut count: i32 = 0;
    let mut error_buf = [0u8; 512];

    let result = unsafe {
        sccapture_get_displays(
            &mut displays_ptr,
            &mut count,
            error_buf.as_mut_ptr(),
            error_buf.len() as i32,
        )
    };

    if result != 0 {
        return Err(AppError::Capture(format!(
            "ScreenCaptureKit: {}", get_error(&error_buf)
        )));
    }

    let displays = if !displays_ptr.is_null() && count > 0 {
        let slice = unsafe { std::slice::from_raw_parts(displays_ptr, count as usize) };
        let vec: Vec<SCDisplayInfo> = slice.iter().map(|d| SCDisplayInfo {
            display_id: d.display_id,
            x: d.x,
            y: d.y,
            width: d.width,
            height: d.height,
        }).collect();
        unsafe { libc_free(displays_ptr as *mut std::ffi::c_void); }
        vec
    } else {
        Vec::new()
    };

    Ok(displays)
}

extern "C" {
    #[link_name = "free"]
    fn libc_free(ptr: *mut std::ffi::c_void);
}

/// Capture a screen region. Coordinates are logical points.
pub fn capture_region(x: f64, y: f64, w: f64, h: f64) -> Result<RgbaImage, AppError> {
    let mut rgba_ptr: *mut u8 = std::ptr::null_mut();
    let mut img_w: u32 = 0;
    let mut img_h: u32 = 0;
    let mut error_buf = [0u8; 512];

    let result = unsafe {
        sccapture_capture_region(
            x, y, w, h,
            &mut rgba_ptr,
            &mut img_w,
            &mut img_h,
            error_buf.as_mut_ptr(),
            error_buf.len() as i32,
        )
    };

    if result != 0 {
        return Err(AppError::Capture(format!(
            "ScreenCaptureKit capture failed: {}", get_error(&error_buf)
        )));
    }

    if rgba_ptr.is_null() || img_w == 0 || img_h == 0 {
        return Err(AppError::Capture("ScreenCaptureKit returned empty image".to_string()));
    }

    let len = (img_w as usize) * (img_h as usize) * 4;
    let pixels = unsafe { std::slice::from_raw_parts(rgba_ptr, len) }.to_vec();
    unsafe { sccapture_free_pixels(rgba_ptr); }

    RgbaImage::from_raw(img_w, img_h, pixels)
        .ok_or_else(|| AppError::Capture("Failed to create image from SCK data".to_string()))
}

/// Capture a full display by ID.
pub fn capture_display(display_id: u32) -> Result<RgbaImage, AppError> {
    let mut rgba_ptr: *mut u8 = std::ptr::null_mut();
    let mut img_w: u32 = 0;
    let mut img_h: u32 = 0;
    let mut error_buf = [0u8; 512];

    let result = unsafe {
        sccapture_capture_display(
            display_id,
            &mut rgba_ptr,
            &mut img_w,
            &mut img_h,
            error_buf.as_mut_ptr(),
            error_buf.len() as i32,
        )
    };

    if result != 0 {
        return Err(AppError::Capture(format!(
            "ScreenCaptureKit display capture failed: {}", get_error(&error_buf)
        )));
    }

    if rgba_ptr.is_null() || img_w == 0 || img_h == 0 {
        return Err(AppError::Capture("ScreenCaptureKit returned empty display image".to_string()));
    }

    let len = (img_w as usize) * (img_h as usize) * 4;
    let pixels = unsafe { std::slice::from_raw_parts(rgba_ptr, len) }.to_vec();
    unsafe { sccapture_free_pixels(rgba_ptr); }

    RgbaImage::from_raw(img_w, img_h, pixels)
        .ok_or_else(|| AppError::Capture("Failed to create image from SCK display data".to_string()))
}

/// Check if screen recording permission is granted.
/// Returns Ok(()) if granted, Err with message if denied.
pub fn check_permission() -> Result<(), AppError> {
    let mut error_buf = [0u8; 512];
    let result = unsafe {
        sccapture_check_permission(error_buf.as_mut_ptr(), error_buf.len() as i32)
    };
    if result != 0 {
        Err(AppError::Capture(format!(
            "Screen recording permission denied: {}", get_error(&error_buf)
        )))
    } else {
        Ok(())
    }
}
