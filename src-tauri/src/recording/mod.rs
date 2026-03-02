pub mod encoder;
pub mod encoder_cpu;
pub mod gif_encoder;
pub mod mp4_muxer;
pub mod pipeline;

#[cfg(target_os = "windows")]
pub mod encoder_win;

#[cfg(target_os = "macos")]
pub mod encoder_mac;

pub use pipeline::{CaptureSource, PipelineConfig, PipelineHandle, RecordingFormat, RecordingRegion};
