use std::path::PathBuf;

use crate::error::AppError;

/// Configuration for initializing a video encoder.
pub struct EncoderConfig {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub bitrate: u32,
    pub output_path: PathBuf,
}

/// Platform-independent video encoder interface.
/// Each platform implements this with GPU-accelerated encoding.
pub trait VideoEncoder: Send {
    /// Initialize the encoder. Called once before any encode calls.
    fn init(&mut self, config: &EncoderConfig) -> Result<(), AppError>;

    /// Encode one RGBA frame.
    fn encode_frame(&mut self, rgba: &[u8], width: u32, height: u32, pts_ms: f64) -> Result<(), AppError>;

    /// Flush remaining frames and finalize the output file.
    fn finish(&mut self) -> Result<(), AppError>;

    /// Human-readable encoder name for logging.
    fn name(&self) -> &str;
}

/// Encoder that tries the platform GPU encoder first, falls back to CPU on failure.
/// This handles cases where the GPU encoder compiles but fails at runtime
/// (missing codecs, driver issues, ObjC bridging problems, etc.).
pub struct FallbackEncoder {
    primary: Option<Box<dyn VideoEncoder>>,
    fallback: Option<Box<dyn VideoEncoder>>,
    active: Option<Box<dyn VideoEncoder>>,
}

impl FallbackEncoder {
    pub fn new(primary: Box<dyn VideoEncoder>, fallback: Box<dyn VideoEncoder>) -> Self {
        Self {
            primary: Some(primary),
            fallback: Some(fallback),
            active: None,
        }
    }
}

impl VideoEncoder for FallbackEncoder {
    fn init(&mut self, config: &EncoderConfig) -> Result<(), AppError> {
        // Try the primary (GPU) encoder first
        if let Some(mut primary) = self.primary.take() {
            match primary.init(config) {
                Ok(()) => {
                    eprintln!("recording: using primary encoder: {}", primary.name());
                    self.active = Some(primary);
                    self.fallback = None; // Don't need fallback
                    return Ok(());
                }
                Err(e) => {
                    eprintln!("recording: primary encoder ({}) failed: {e}", primary.name());
                    eprintln!("recording: falling back to CPU encoder");
                    // primary is dropped here
                }
            }
        }

        // Fall back to CPU encoder
        if let Some(mut fallback) = self.fallback.take() {
            fallback.init(config)?;
            eprintln!("recording: using fallback encoder: {}", fallback.name());
            self.active = Some(fallback);
            Ok(())
        } else {
            Err(AppError::Recording("No encoder available".to_string()))
        }
    }

    fn encode_frame(&mut self, rgba: &[u8], width: u32, height: u32, pts_ms: f64) -> Result<(), AppError> {
        self.active.as_mut()
            .ok_or_else(|| AppError::Recording("Encoder not initialized".to_string()))?
            .encode_frame(rgba, width, height, pts_ms)
    }

    fn finish(&mut self) -> Result<(), AppError> {
        self.active.as_mut()
            .ok_or_else(|| AppError::Recording("Encoder not initialized".to_string()))?
            .finish()
    }

    fn name(&self) -> &str {
        self.active.as_ref().map(|e| e.name()).unwrap_or("FallbackEncoder (not initialized)")
    }
}

/// Create the best available video encoder for the current platform.
/// Uses GPU-accelerated encoder with automatic CPU fallback on failure.
pub fn create_encoder() -> Box<dyn VideoEncoder> {
    #[cfg(target_os = "windows")]
    {
        // Windows Media Foundation handles its own fallback (software encoder built-in)
        Box::new(super::encoder_win::MediaFoundationEncoder::new())
    }

    #[cfg(target_os = "macos")]
    {
        Box::new(FallbackEncoder::new(
            Box::new(super::encoder_mac::AVAssetWriterEncoder::new()),
            Box::new(super::encoder_cpu::CpuEncoder::new()),
        ))
    }

    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        Box::new(super::encoder_cpu::CpuEncoder::new())
    }
}
