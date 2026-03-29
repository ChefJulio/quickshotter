use std::fs::File;
use std::io::BufWriter;
use std::path::Path;

use crate::error::AppError;

/// GIF recording configuration.
pub struct GifConfig {
    pub max_width: u32,
    pub frame_delay_cs: u16, // centiseconds (1/100s) per frame
}

/// GIF encoder using the `gif` crate.
/// Handles downscaling, color quantization, and frame-by-frame encoding.
pub struct GifEncoder {
    writer: Option<gif::Encoder<BufWriter<File>>>,
    max_width: u32,
    frame_delay_cs: u16,
    output_width: u32,
    output_height: u32,
    frame_count: u32,
    // Reusable buffers to avoid per-frame allocations
    rgb_buf: Vec<u8>,
    index_buf: Vec<u8>,
}

impl GifEncoder {
    pub fn new() -> Self {
        Self {
            writer: None,
            max_width: 800,
            frame_delay_cs: 10,
            output_width: 0,
            output_height: 0,
            frame_count: 0,
            rgb_buf: Vec::new(),
            index_buf: Vec::new(),
        }
    }

    /// Initialize the encoder. The first frame determines the output dimensions.
    /// We defer creating the gif::Encoder until we know width/height from the first frame.
    pub fn init(&mut self, config: &GifConfig, _output_path: &Path) -> Result<(), AppError> {
        self.max_width = config.max_width;
        self.frame_delay_cs = config.frame_delay_cs;
        Ok(())
    }

    /// Add an RGBA frame. The encoder handles downscaling and quantization.
    pub fn add_frame(
        &mut self,
        rgba: &[u8],
        width: u32,
        height: u32,
        output_path: &Path,
    ) -> Result<(), AppError> {
        // Downscale if needed — borrow the data instead of cloning
        let (scaled_rgba, sw, sh) = if width > self.max_width {
            let scale = self.max_width as f64 / width as f64;
            let new_w = self.max_width;
            let new_h = (height as f64 * scale).round() as u32;
            // Wrap without copying — from_raw requires owned Vec but we can
            // use image::imageops::resize on a view instead.
            let src = image::RgbaImage::from_raw(width, height, rgba.to_vec())
                .ok_or_else(|| AppError::Recording("Invalid RGBA data".to_string()))?;
            let resized = image::imageops::resize(&src, new_w, new_h, image::imageops::FilterType::Triangle);
            (std::borrow::Cow::Owned(resized.into_raw()), new_w, new_h)
        } else {
            (std::borrow::Cow::Borrowed(rgba), width, height)
        };

        // Create encoder on first frame (now we know dimensions)
        if self.writer.is_none() {
            self.output_width = sw;
            self.output_height = sh;
            let file = File::create(output_path)
                .map_err(|e| AppError::Recording(format!("Failed to create GIF file: {e}")))?;
            let buf = BufWriter::new(file);
            let mut encoder = gif::Encoder::new(buf, sw as u16, sh as u16, &[])
                .map_err(|e| AppError::Recording(format!("Failed to init GIF encoder: {e}")))?;
            encoder.set_repeat(gif::Repeat::Infinite)
                .map_err(|e| AppError::Recording(format!("Failed to set GIF repeat: {e}")))?;
            self.writer = Some(encoder);
        }

        // Quantize RGBA to 256-color palette — reuse buffers across frames
        let pixel_count = (sw * sh) as usize;
        self.rgb_buf.clear();
        self.rgb_buf.reserve(pixel_count * 3);
        for pixel in scaled_rgba.chunks_exact(4) {
            self.rgb_buf.push(pixel[0]);
            self.rgb_buf.push(pixel[1]);
            self.rgb_buf.push(pixel[2]);
        }

        let nq = color_quant::NeuQuant::new(10, 256, &self.rgb_buf);
        let palette = nq.color_map_rgb();

        // Map pixels to palette indices — reuse buffer
        self.index_buf.clear();
        self.index_buf.reserve(pixel_count);
        for pixel in self.rgb_buf.chunks_exact(3) {
            self.index_buf.push(nq.index_of(&[pixel[0], pixel[1], pixel[2], 255]) as u8);
        }

        let mut frame = gif::Frame::default();
        frame.width = sw as u16;
        frame.height = sh as u16;
        frame.delay = self.frame_delay_cs;
        frame.palette = Some(palette);
        frame.buffer = std::borrow::Cow::Borrowed(&self.index_buf);

        if let Some(ref mut writer) = self.writer {
            writer.write_frame(&frame)
                .map_err(|e| AppError::Recording(format!("Failed to write GIF frame: {e}")))?;
        }

        self.frame_count += 1;
        Ok(())
    }

    pub fn finish(&mut self) -> Result<(), AppError> {
        // Drop the writer to flush and finalize
        self.writer.take();
        Ok(())
    }

    pub fn frame_count(&self) -> u32 {
        self.frame_count
    }
}
