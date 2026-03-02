use std::fs::File;
use std::io::BufWriter;

use crate::error::AppError;
use super::encoder::{EncoderConfig, VideoEncoder};
use super::mp4_muxer::Mp4Muxer;

/// CPU-based H.264 encoder using openh264 + minimal MP4 muxer.
/// Used as a fallback when platform GPU encoders are unavailable.
pub struct CpuEncoder {
    encoder: Option<openh264::encoder::Encoder>,
    muxer: Option<Mp4Muxer>,
    output_path: Option<std::path::PathBuf>,
}

impl CpuEncoder {
    pub fn new() -> Self {
        Self {
            encoder: None,
            muxer: None,
            output_path: None,
        }
    }
}

impl VideoEncoder for CpuEncoder {
    fn init(&mut self, config: &EncoderConfig) -> Result<(), AppError> {
        // Ensure even dimensions (openh264 requirement)
        let w = config.width & !1;
        let h = config.height & !1;

        let enc_config = openh264::encoder::EncoderConfig::new()
            .set_bitrate_bps(config.bitrate)
            .max_frame_rate(config.fps as f32)
            .rate_control_mode(openh264::encoder::RateControlMode::Bitrate);

        let api = openh264::OpenH264API::from_source();
        let encoder = openh264::encoder::Encoder::with_api_config(api, enc_config)
            .map_err(|e| AppError::Recording(format!("openh264 init failed: {e}")))?;

        let muxer = Mp4Muxer::new(w, h, config.fps);

        self.encoder = Some(encoder);
        self.muxer = Some(muxer);
        self.output_path = Some(config.output_path.clone());

        Ok(())
    }

    fn encode_frame(&mut self, rgba: &[u8], width: u32, height: u32, _pts_ms: f64) -> Result<(), AppError> {
        let encoder = self.encoder.as_mut()
            .ok_or_else(|| AppError::Recording("CPU encoder not initialized".to_string()))?;
        let muxer = self.muxer.as_mut()
            .ok_or_else(|| AppError::Recording("MP4 muxer not initialized".to_string()))?;

        // Ensure even dimensions
        let w = (width & !1) as usize;
        let h = (height & !1) as usize;

        // Convert RGBA to YUV via openh264's built-in conversion
        let src = openh264::formats::RgbaSliceU8::new(rgba, (w, h));
        let yuv = openh264::formats::YUVBuffer::from_rgb_source(src);

        // Encode to H.264
        let bitstream = encoder.encode(&yuv)
            .map_err(|e| AppError::Recording(format!("openh264 encode failed: {e}")))?;

        let nal_data = bitstream.to_vec();
        if !nal_data.is_empty() {
            let is_keyframe = matches!(
                bitstream.frame_type(),
                openh264::encoder::FrameType::IDR | openh264::encoder::FrameType::I
            );
            muxer.add_frame(&nal_data, is_keyframe);
        }

        Ok(())
    }

    fn finish(&mut self) -> Result<(), AppError> {
        let muxer = self.muxer.take()
            .ok_or_else(|| AppError::Recording("MP4 muxer not initialized".to_string()))?;
        let output_path = self.output_path.as_ref()
            .ok_or_else(|| AppError::Recording("No output path".to_string()))?;

        let file = File::create(output_path)
            .map_err(|e| AppError::Recording(format!("Failed to create MP4 file: {e}")))?;
        let mut writer = BufWriter::new(file);
        muxer.finish(&mut writer)?;

        self.encoder = None;
        Ok(())
    }

    fn name(&self) -> &str {
        "openh264 (CPU)"
    }
}
