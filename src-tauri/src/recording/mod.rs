pub mod encoder;
pub mod gif_encoder;
pub mod pipeline;

// CPU encoder + MP4 muxer are used on macOS/Linux as fallback for GPU encoder.
// On Windows, Media Foundation handles everything, so these are compiled but unused.
#[cfg_attr(target_os = "windows", allow(dead_code))]
pub mod encoder_cpu;
#[cfg_attr(target_os = "windows", allow(dead_code))]
pub mod mp4_muxer;

#[cfg(target_os = "windows")]
pub mod encoder_win;

#[cfg(target_os = "macos")]
pub mod encoder_mac;

pub use pipeline::{CaptureSource, PipelineConfig, RecordingFormat, RecordingRegion};
