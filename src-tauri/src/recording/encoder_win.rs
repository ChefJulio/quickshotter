use crate::error::AppError;
use super::encoder::{EncoderConfig, VideoEncoder};

use windows::core::PCWSTR;
use windows::Win32::Media::MediaFoundation::{
    IMFAttributes, IMFSinkWriter,
    MFCreateAttributes, MFCreateMediaType, MFCreateMemoryBuffer, MFCreateSample,
    MFCreateSinkWriterFromURL, MFShutdown, MFStartup,
    MF_API_VERSION, MF_MT_AVG_BITRATE, MF_MT_FRAME_RATE, MF_MT_FRAME_SIZE,
    MF_MT_INTERLACE_MODE, MF_MT_MAJOR_TYPE, MF_MT_PIXEL_ASPECT_RATIO, MF_MT_SUBTYPE,
    MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS, MF_SDK_VERSION, MFSTARTUP_NOSOCKET,
    MFMediaType_Video, MFVideoFormat_H264, MFVideoFormat_RGB32,
    MFVideoInterlace_Progressive,
};
use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED};

/// Windows Media Foundation H.264 encoder.
/// Uses IMFSinkWriter which auto-selects the best available hardware encoder
/// (NVENC, AMF, QuickSync, D3D11) with built-in software fallback.
/// Handles both encoding and MP4 muxing in one step.
pub struct MediaFoundationEncoder {
    sink_writer: Option<IMFSinkWriter>,
    stream_index: u32,
    frame_duration_100ns: i64,
    com_initialized: bool,
    mf_initialized: bool,
    frame_count: u32,
}

// Safety: MediaFoundationEncoder is created, used, and dropped entirely on the encoder thread.
// COM objects are apartment-threaded but we ensure single-thread usage.
unsafe impl Send for MediaFoundationEncoder {}

impl MediaFoundationEncoder {
    pub fn new() -> Self {
        Self {
            sink_writer: None,
            stream_index: 0,
            frame_duration_100ns: 0,
            com_initialized: false,
            mf_initialized: false,
            frame_count: 0,
        }
    }
}

impl VideoEncoder for MediaFoundationEncoder {
    fn init(&mut self, config: &EncoderConfig) -> Result<(), AppError> {
        unsafe {
            // Initialize COM (apartment-threaded for the encoder thread)
            CoInitializeEx(None, COINIT_APARTMENTTHREADED)
                .ok()
                .map_err(|e| AppError::Recording(format!("COM init failed: {e}")))?;
            self.com_initialized = true;

            // Initialize Media Foundation
            let mf_version = (MF_SDK_VERSION << 16) | MF_API_VERSION;
            MFStartup(mf_version, MFSTARTUP_NOSOCKET)
                .map_err(|e| AppError::Recording(format!("MFStartup failed: {e}")))?;
            self.mf_initialized = true;

            // Create attributes with hardware encoding enabled
            let mut attrs: Option<IMFAttributes> = None;
            MFCreateAttributes(&mut attrs, 1)
                .map_err(|e| AppError::Recording(format!("MFCreateAttributes failed: {e}")))?;
            let attrs = attrs.ok_or_else(|| AppError::Recording("Null attributes".to_string()))?;
            attrs.SetUINT32(&MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS, 1)
                .map_err(|e| AppError::Recording(format!("SetUINT32 hw transforms failed: {e}")))?;

            // Convert output path to wide string
            let path_str = config.output_path.to_string_lossy();
            let path_wide: Vec<u16> = path_str.encode_utf16().chain(Some(0)).collect();

            let sink_writer = MFCreateSinkWriterFromURL(
                PCWSTR(path_wide.as_ptr()),
                None,
                &attrs,
            ).map_err(|e| AppError::Recording(format!("MFCreateSinkWriterFromURL failed: {e}")))?;

            let w = config.width;
            let h = config.height;
            let fps = config.fps;
            let bitrate = config.bitrate;

            // Output type: H.264
            let out_type = MFCreateMediaType()
                .map_err(|e| AppError::Recording(format!("MFCreateMediaType (output) failed: {e}")))?;
            out_type.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)
                .map_err(|e| AppError::Recording(format!("SetGUID major type failed: {e}")))?;
            out_type.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_H264)
                .map_err(|e| AppError::Recording(format!("SetGUID H264 subtype failed: {e}")))?;
            out_type.SetUINT32(&MF_MT_AVG_BITRATE, bitrate)
                .map_err(|e| AppError::Recording(format!("SetUINT32 bitrate failed: {e}")))?;
            out_type.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)
                .map_err(|e| AppError::Recording(format!("SetUINT32 interlace failed: {e}")))?;
            out_type.SetUINT64(&MF_MT_FRAME_SIZE, ((w as u64) << 32) | h as u64)
                .map_err(|e| AppError::Recording(format!("SetUINT64 frame size failed: {e}")))?;
            out_type.SetUINT64(&MF_MT_FRAME_RATE, ((fps as u64) << 32) | 1u64)
                .map_err(|e| AppError::Recording(format!("SetUINT64 frame rate failed: {e}")))?;
            out_type.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, (1u64 << 32) | 1u64)
                .map_err(|e| AppError::Recording(format!("SetUINT64 pixel aspect failed: {e}")))?;

            let stream_index = sink_writer.AddStream(&out_type)
                .map_err(|e| AppError::Recording(format!("AddStream failed: {e}")))?;

            // Input type: RGB32 (BGRA byte order on Windows)
            let in_type = MFCreateMediaType()
                .map_err(|e| AppError::Recording(format!("MFCreateMediaType (input) failed: {e}")))?;
            in_type.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)
                .map_err(|e| AppError::Recording(format!("SetGUID major type (input) failed: {e}")))?;
            in_type.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_RGB32)
                .map_err(|e| AppError::Recording(format!("SetGUID RGB32 subtype failed: {e}")))?;
            in_type.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)
                .map_err(|e| AppError::Recording(format!("SetUINT32 interlace (input) failed: {e}")))?;
            in_type.SetUINT64(&MF_MT_FRAME_SIZE, ((w as u64) << 32) | h as u64)
                .map_err(|e| AppError::Recording(format!("SetUINT64 frame size (input) failed: {e}")))?;
            in_type.SetUINT64(&MF_MT_FRAME_RATE, ((fps as u64) << 32) | 1u64)
                .map_err(|e| AppError::Recording(format!("SetUINT64 frame rate (input) failed: {e}")))?;
            in_type.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, (1u64 << 32) | 1u64)
                .map_err(|e| AppError::Recording(format!("SetUINT64 pixel aspect (input) failed: {e}")))?;

            sink_writer.SetInputMediaType(stream_index, &in_type, None)
                .map_err(|e| AppError::Recording(format!("SetInputMediaType failed: {e}")))?;

            sink_writer.BeginWriting()
                .map_err(|e| AppError::Recording(format!("BeginWriting failed: {e}")))?;

            self.sink_writer = Some(sink_writer);
            self.stream_index = stream_index;
            // Frame duration in 100-nanosecond units
            self.frame_duration_100ns = (10_000_000i64 / fps as i64).max(1);
        }

        Ok(())
    }

    fn encode_frame(&mut self, rgba: &[u8], width: u32, height: u32, pts_ms: f64) -> Result<(), AppError> {
        let sink_writer = self.sink_writer.as_ref()
            .ok_or_else(|| AppError::Recording("Encoder not initialized".to_string()))?;

        let expected_len = (width as usize) * (height as usize) * 4;
        if rgba.len() < expected_len {
            return Err(AppError::Recording(format!(
                "RGBA buffer too small: got {} bytes, expected {} ({}x{}x4)",
                rgba.len(), expected_len, width, height
            )));
        }

        unsafe {
            let buffer_size = (width * height * 4) as u32;
            let media_buffer = MFCreateMemoryBuffer(buffer_size)
                .map_err(|e| AppError::Recording(format!("MFCreateMemoryBuffer failed: {e}")))?;

            // Lock buffer, convert RGBA -> BGRA and flip vertically (MF expects bottom-up RGB32)
            let mut data_ptr = std::ptr::null_mut();
            media_buffer.Lock(&mut data_ptr, None, None)
                .map_err(|e| AppError::Recording(format!("Buffer Lock failed: {e}")))?;

            let stride = (width * 4) as usize;
            for y in 0..height as usize {
                // MF RGB32 is bottom-up: first row in buffer = last row of image
                let src_row = &rgba[y * stride..(y + 1) * stride];
                let dst_offset = (height as usize - 1 - y) * stride;
                let dst_row = std::slice::from_raw_parts_mut(data_ptr.add(dst_offset), stride);
                // RGBA -> BGRA: swap R and B channels
                for x in 0..width as usize {
                    let si = x * 4;
                    dst_row[si] = src_row[si + 2];     // B
                    dst_row[si + 1] = src_row[si + 1]; // G
                    dst_row[si + 2] = src_row[si];     // R
                    dst_row[si + 3] = 0;               // pad (ignored for RGB32)
                }
            }

            media_buffer.Unlock()
                .map_err(|e| AppError::Recording(format!("Buffer Unlock failed: {e}")))?;
            media_buffer.SetCurrentLength(buffer_size)
                .map_err(|e| AppError::Recording(format!("SetCurrentLength failed: {e}")))?;

            let sample = MFCreateSample()
                .map_err(|e| AppError::Recording(format!("MFCreateSample failed: {e}")))?;
            sample.AddBuffer(&media_buffer)
                .map_err(|e| AppError::Recording(format!("AddBuffer failed: {e}")))?;

            // Convert pts from milliseconds to 100-nanosecond units
            let timestamp_100ns = (pts_ms * 10_000.0) as i64;
            sample.SetSampleTime(timestamp_100ns)
                .map_err(|e| AppError::Recording(format!("SetSampleTime failed: {e}")))?;
            sample.SetSampleDuration(self.frame_duration_100ns)
                .map_err(|e| AppError::Recording(format!("SetSampleDuration failed: {e}")))?;

            sink_writer.WriteSample(self.stream_index, &sample)
                .map_err(|e| AppError::Recording(format!("WriteSample failed: {e}")))?;
            self.frame_count += 1;
        }

        Ok(())
    }

    fn finish(&mut self) -> Result<(), AppError> {
        eprintln!("recording: MF encoder finishing ({} frames written via WriteSample)", self.frame_count);
        unsafe {
            if let Some(ref writer) = self.sink_writer {
                writer.Finalize()
                    .map_err(|e| AppError::Recording(format!("Finalize failed: {e}")))?;
                eprintln!("recording: MF Finalize() succeeded");
            }
            // Drop sink writer before MFShutdown to ensure file is fully flushed
            self.sink_writer = None;

            if self.mf_initialized {
                MFShutdown().ok();
                self.mf_initialized = false;
            }
            if self.com_initialized {
                CoUninitialize();
                self.com_initialized = false;
            }
        }
        Ok(())
    }

    fn name(&self) -> &str {
        "Media Foundation H.264 (GPU)"
    }
}

impl Drop for MediaFoundationEncoder {
    fn drop(&mut self) {
        unsafe {
            // Ensure cleanup even if finish() wasn't called
            self.sink_writer = None;
            if self.mf_initialized {
                MFShutdown().ok();
            }
            if self.com_initialized {
                CoUninitialize();
            }
        }
    }
}
