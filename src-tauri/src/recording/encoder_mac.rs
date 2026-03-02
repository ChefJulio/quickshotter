use std::ffi::CString;
use std::ptr;

use crate::error::AppError;
use super::encoder::{EncoderConfig, VideoEncoder};

// FFI declarations for the Objective-C wrapper (avwriter.m).
// These functions are compiled by cc::Build in build.rs.
#[repr(C)]
struct AVWriterHandle {
    _opaque: [u8; 0],
}

extern "C" {
    fn avwriter_create(
        path: *const i8,
        width: u32,
        height: u32,
        fps: u32,
        bitrate: u32,
    ) -> *mut AVWriterHandle;

    fn avwriter_append_frame(
        handle: *mut AVWriterHandle,
        rgba: *const u8,
        width: u32,
        height: u32,
        pts_ms: f64,
    ) -> i32;

    fn avwriter_finish(handle: *mut AVWriterHandle) -> i32;
    fn avwriter_destroy(handle: *mut AVWriterHandle);
    fn avwriter_error(handle: *mut AVWriterHandle) -> *const i8;
}

/// Read the error string from a handle, returning a Rust String.
unsafe fn get_error(handle: *mut AVWriterHandle) -> String {
    let ptr = avwriter_error(handle);
    if ptr.is_null() {
        return "unknown error".to_string();
    }
    std::ffi::CStr::from_ptr(ptr)
        .to_string_lossy()
        .into_owned()
}

/// macOS AVAssetWriter + VideoToolbox H.264 encoder.
/// Uses hardware-accelerated encoding via Apple's VideoToolbox framework.
/// AVAssetWriter handles both encoding and MP4 muxing.
pub struct AVAssetWriterEncoder {
    handle: *mut AVWriterHandle,
    frame_count: u32,
}

// Safety: The handle wraps ObjC objects that are used single-threaded
// on the encoder thread. The ObjC runtime handles thread safety internally.
unsafe impl Send for AVAssetWriterEncoder {}

impl AVAssetWriterEncoder {
    pub fn new() -> Self {
        Self {
            handle: ptr::null_mut(),
            frame_count: 0,
        }
    }
}

impl VideoEncoder for AVAssetWriterEncoder {
    fn init(&mut self, config: &EncoderConfig) -> Result<(), AppError> {
        let path = CString::new(config.output_path.to_string_lossy().as_bytes())
            .map_err(|e| AppError::Recording(format!("Invalid path: {e}")))?;

        let handle = unsafe {
            avwriter_create(
                path.as_ptr(),
                config.width,
                config.height,
                config.fps,
                config.bitrate,
            )
        };

        if handle.is_null() {
            return Err(AppError::Recording(
                "AVAssetWriter creation returned null".to_string(),
            ));
        }

        // Check if creation had an error (handle allocated but writer failed)
        let err = unsafe { get_error(handle) };
        if !err.is_empty() {
            unsafe { avwriter_destroy(handle); }
            return Err(AppError::Recording(format!(
                "AVAssetWriter init failed: {err}"
            )));
        }

        self.handle = handle;
        Ok(())
    }

    fn encode_frame(
        &mut self,
        rgba: &[u8],
        width: u32,
        height: u32,
        pts_ms: f64,
    ) -> Result<(), AppError> {
        if self.handle.is_null() {
            return Err(AppError::Recording(
                "AVAssetWriter encoder not initialized".to_string(),
            ));
        }

        let result = unsafe {
            avwriter_append_frame(self.handle, rgba.as_ptr(), width, height, pts_ms)
        };

        if result != 0 {
            let err = unsafe { get_error(self.handle) };
            return Err(AppError::Recording(format!(
                "AVAssetWriter appendFrame failed: {err}"
            )));
        }

        self.frame_count += 1;
        Ok(())
    }

    fn finish(&mut self) -> Result<(), AppError> {
        if self.handle.is_null() {
            return Ok(());
        }

        eprintln!(
            "recording: AVAssetWriter finishing ({} frames)",
            self.frame_count
        );

        let result = unsafe { avwriter_finish(self.handle) };
        if result != 0 {
            let err = unsafe { get_error(self.handle) };
            // Still destroy the handle
            unsafe { avwriter_destroy(self.handle); }
            self.handle = ptr::null_mut();
            return Err(AppError::Recording(format!(
                "AVAssetWriter finish failed: {err}"
            )));
        }

        unsafe { avwriter_destroy(self.handle); }
        self.handle = ptr::null_mut();
        eprintln!("recording: AVAssetWriter finished successfully");
        Ok(())
    }

    fn name(&self) -> &str {
        "AVAssetWriter + VideoToolbox H.264 (GPU)"
    }
}

impl Drop for AVAssetWriterEncoder {
    fn drop(&mut self) {
        if !self.handle.is_null() {
            unsafe { avwriter_destroy(self.handle); }
            self.handle = ptr::null_mut();
        }
    }
}
