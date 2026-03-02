//! Minimal MP4 muxer for a single H.264 video track.
//! Writes ISOBMFF (ISO 14496-12) containers.
//!
//! Strategy: accumulate all NAL units in memory, then write the entire
//! MP4 file on finish(). This avoids seeking/patching but limits file size
//! to available memory. For screen recordings (typically < 1GB), this is fine.

use std::io::Write;
use crate::error::AppError;

/// A single encoded frame's NAL data.
struct FrameData {
    nal_units: Vec<Vec<u8>>,
    is_keyframe: bool,
}

/// Minimal MP4 muxer for single-track H.264 video.
pub struct Mp4Muxer {
    width: u32,
    height: u32,
    fps: u32,
    timescale: u32,
    frames: Vec<FrameData>,
    sps: Option<Vec<u8>>,
    pps: Option<Vec<u8>>,
}

impl Mp4Muxer {
    pub fn new(width: u32, height: u32, fps: u32) -> Self {
        Self {
            width,
            height,
            fps,
            timescale: 90000, // standard video timescale
            frames: Vec::new(),
            sps: None,
            pps: None,
        }
    }

    /// Add an Annex B encoded frame.
    /// Parses NAL units, extracts SPS/PPS, converts to AVCC format.
    pub fn add_frame(&mut self, annex_b_data: &[u8], is_keyframe: bool) {
        let nals = split_annex_b(annex_b_data);
        let mut frame_nals = Vec::new();

        for nal in nals {
            if nal.is_empty() { continue; }
            let nal_type = nal[0] & 0x1F;
            match nal_type {
                7 => { self.sps = Some(nal.to_vec()); } // SPS
                8 => { self.pps = Some(nal.to_vec()); } // PPS
                _ => { frame_nals.push(nal.to_vec()); }
            }
        }

        if !frame_nals.is_empty() {
            self.frames.push(FrameData {
                nal_units: frame_nals,
                is_keyframe,
            });
        }
    }

    /// Write the complete MP4 file.
    pub fn finish<W: Write>(&self, out: &mut W) -> Result<(), AppError> {
        let sps = self.sps.as_ref()
            .ok_or_else(|| AppError::Recording("No SPS found in H.264 stream".to_string()))?;
        let pps = self.pps.as_ref()
            .ok_or_else(|| AppError::Recording("No PPS found in H.264 stream".to_string()))?;

        if self.frames.is_empty() {
            return Err(AppError::Recording("No frames to write".to_string()));
        }

        // Build sample data (AVCC format: 4-byte length prefix per NAL unit)
        let mut mdat_payload = Vec::new();
        let mut sample_sizes: Vec<u32> = Vec::new();
        let mut sync_samples: Vec<u32> = Vec::new(); // 1-indexed

        for (i, frame) in self.frames.iter().enumerate() {
            let sample_start = mdat_payload.len();
            for nal in &frame.nal_units {
                let len = nal.len() as u32;
                mdat_payload.extend_from_slice(&len.to_be_bytes());
                mdat_payload.extend_from_slice(nal);
            }
            sample_sizes.push((mdat_payload.len() - sample_start) as u32);
            if frame.is_keyframe {
                sync_samples.push((i + 1) as u32);
            }
        }

        let frame_count = self.frames.len() as u32;
        let duration_ts = frame_count as u64 * self.timescale as u64 / self.fps as u64;
        let sample_duration = self.timescale / self.fps;

        // Build avcC box data
        let avcc_data = build_avcc(sps, pps);

        // Calculate moov size to determine mdat offset
        let moov_data = self.build_moov(
            &avcc_data, &sample_sizes, &sync_samples,
            frame_count, duration_ts, sample_duration,
        );

        // ftyp box
        let ftyp = build_ftyp();

        // mdat box: 8-byte header + payload
        let mdat_size = 8 + mdat_payload.len() as u32;

        // Write: ftyp, moov, mdat
        // mdat_offset = ftyp.len() + moov.len() + 8 (mdat header)
        let mdat_data_offset = (ftyp.len() + moov_data.len() + 8) as u32;

        // Patch stco (chunk offset) in moov to point to mdat payload
        let moov_data = self.build_moov_with_offset(
            &avcc_data, &sample_sizes, &sync_samples,
            frame_count, duration_ts, sample_duration,
            mdat_data_offset,
        );

        out.write_all(&ftyp).map_err(io_err)?;
        out.write_all(&moov_data).map_err(io_err)?;

        // mdat header
        out.write_all(&mdat_size.to_be_bytes()).map_err(io_err)?;
        out.write_all(b"mdat").map_err(io_err)?;
        out.write_all(&mdat_payload).map_err(io_err)?;

        Ok(())
    }

    fn build_moov(
        &self,
        avcc_data: &[u8],
        sample_sizes: &[u32],
        sync_samples: &[u32],
        frame_count: u32,
        duration_ts: u64,
        sample_duration: u32,
    ) -> Vec<u8> {
        self.build_moov_with_offset(
            avcc_data, sample_sizes, sync_samples,
            frame_count, duration_ts, sample_duration, 0,
        )
    }

    fn build_moov_with_offset(
        &self,
        avcc_data: &[u8],
        sample_sizes: &[u32],
        sync_samples: &[u32],
        frame_count: u32,
        duration_ts: u64,
        sample_duration: u32,
        mdat_data_offset: u32,
    ) -> Vec<u8> {
        let duration_s = duration_ts as f64 / self.timescale as f64;
        let mvhd_timescale = 1000u32;
        let mvhd_duration = (duration_s * mvhd_timescale as f64) as u32;

        let mvhd = build_mvhd(mvhd_timescale, mvhd_duration);
        let trak = self.build_trak(
            avcc_data, sample_sizes, sync_samples,
            frame_count, duration_ts, sample_duration,
            mdat_data_offset, mvhd_duration,
        );
        wrap_box(b"moov", &[&mvhd, &trak])
    }

    fn build_trak(
        &self,
        avcc_data: &[u8],
        sample_sizes: &[u32],
        sync_samples: &[u32],
        frame_count: u32,
        duration_ts: u64,
        sample_duration: u32,
        mdat_data_offset: u32,
        mvhd_duration: u32,
    ) -> Vec<u8> {
        let tkhd = build_tkhd(self.width, self.height, mvhd_duration);
        let mdia = self.build_mdia(
            avcc_data, sample_sizes, sync_samples,
            frame_count, duration_ts, sample_duration,
            mdat_data_offset,
        );
        wrap_box(b"trak", &[&tkhd, &mdia])
    }

    fn build_mdia(
        &self,
        avcc_data: &[u8],
        sample_sizes: &[u32],
        sync_samples: &[u32],
        frame_count: u32,
        duration_ts: u64,
        sample_duration: u32,
        mdat_data_offset: u32,
    ) -> Vec<u8> {
        let mdhd = build_mdhd(self.timescale, duration_ts as u32);
        let hdlr = build_hdlr_video();
        let minf = self.build_minf(
            avcc_data, sample_sizes, sync_samples,
            frame_count, sample_duration, mdat_data_offset,
        );
        wrap_box(b"mdia", &[&mdhd, &hdlr, &minf])
    }

    fn build_minf(
        &self,
        avcc_data: &[u8],
        sample_sizes: &[u32],
        sync_samples: &[u32],
        frame_count: u32,
        sample_duration: u32,
        mdat_data_offset: u32,
    ) -> Vec<u8> {
        let vmhd = build_vmhd();
        let dinf = build_dinf();
        let stbl = self.build_stbl(
            avcc_data, sample_sizes, sync_samples,
            frame_count, sample_duration, mdat_data_offset,
        );
        wrap_box(b"minf", &[&vmhd, &dinf, &stbl])
    }

    fn build_stbl(
        &self,
        avcc_data: &[u8],
        sample_sizes: &[u32],
        sync_samples: &[u32],
        frame_count: u32,
        sample_duration: u32,
        mdat_data_offset: u32,
    ) -> Vec<u8> {
        let stsd = build_stsd_avc1(self.width, self.height, avcc_data);
        let stts = build_stts(frame_count, sample_duration);
        let stsc = build_stsc(frame_count);
        let stsz = build_stsz(sample_sizes);
        let stco = build_stco(mdat_data_offset);
        let stss = build_stss(sync_samples);
        wrap_box(b"stbl", &[&stsd, &stts, &stsc, &stsz, &stco, &stss])
    }
}

// -- Box builders --

fn wrap_box(tag: &[u8; 4], children: &[&[u8]]) -> Vec<u8> {
    let content_size: usize = children.iter().map(|c| c.len()).sum();
    let total_size = 8 + content_size;
    let mut buf = Vec::with_capacity(total_size);
    buf.extend_from_slice(&(total_size as u32).to_be_bytes());
    buf.extend_from_slice(tag);
    for child in children {
        buf.extend_from_slice(child);
    }
    buf
}

fn build_ftyp() -> Vec<u8> {
    let mut buf = Vec::new();
    let content = b"isom\x00\x00\x02\x00isomiso2avc1mp41";
    let size = 8u32 + content.len() as u32;
    buf.extend_from_slice(&size.to_be_bytes());
    buf.extend_from_slice(b"ftyp");
    buf.extend_from_slice(content);
    buf
}

fn build_mvhd(timescale: u32, duration: u32) -> Vec<u8> {
    let mut buf = Vec::with_capacity(116);
    let size = 108u32 + 8;
    buf.extend_from_slice(&size.to_be_bytes());
    buf.extend_from_slice(b"mvhd");
    buf.extend_from_slice(&[0u8; 4]); // version + flags
    buf.extend_from_slice(&[0u8; 4]); // creation_time
    buf.extend_from_slice(&[0u8; 4]); // modification_time
    buf.extend_from_slice(&timescale.to_be_bytes());
    buf.extend_from_slice(&duration.to_be_bytes());
    buf.extend_from_slice(&0x00010000u32.to_be_bytes()); // rate = 1.0
    buf.extend_from_slice(&0x0100u16.to_be_bytes()); // volume = 1.0
    buf.extend_from_slice(&[0u8; 10]); // reserved
    // Unity matrix (3x3 fixed-point)
    for &val in &[
        0x00010000u32, 0, 0, 0, 0x00010000, 0, 0, 0, 0x40000000,
    ] {
        buf.extend_from_slice(&val.to_be_bytes());
    }
    buf.extend_from_slice(&[0u8; 24]); // pre_defined
    buf.extend_from_slice(&2u32.to_be_bytes()); // next_track_ID
    buf
}

fn build_tkhd(width: u32, height: u32, duration: u32) -> Vec<u8> {
    let mut buf = Vec::new();
    let size = 92u32 + 8;
    buf.extend_from_slice(&size.to_be_bytes());
    buf.extend_from_slice(b"tkhd");
    buf.extend_from_slice(&[0, 0, 0, 3]); // version=0, flags=track_enabled|track_in_movie
    buf.extend_from_slice(&[0u8; 4]); // creation_time
    buf.extend_from_slice(&[0u8; 4]); // modification_time
    buf.extend_from_slice(&1u32.to_be_bytes()); // track_ID
    buf.extend_from_slice(&[0u8; 4]); // reserved
    buf.extend_from_slice(&duration.to_be_bytes());
    buf.extend_from_slice(&[0u8; 8]); // reserved
    buf.extend_from_slice(&0u16.to_be_bytes()); // layer
    buf.extend_from_slice(&0u16.to_be_bytes()); // alternate_group
    buf.extend_from_slice(&0u16.to_be_bytes()); // volume (0 for video)
    buf.extend_from_slice(&0u16.to_be_bytes()); // reserved
    // Unity matrix
    for &val in &[
        0x00010000u32, 0, 0, 0, 0x00010000, 0, 0, 0, 0x40000000,
    ] {
        buf.extend_from_slice(&val.to_be_bytes());
    }
    // width and height as 16.16 fixed-point
    buf.extend_from_slice(&((width as u32) << 16).to_be_bytes());
    buf.extend_from_slice(&((height as u32) << 16).to_be_bytes());
    buf
}

fn build_mdhd(timescale: u32, duration: u32) -> Vec<u8> {
    let mut buf = Vec::new();
    let size = 32u32;
    buf.extend_from_slice(&size.to_be_bytes());
    buf.extend_from_slice(b"mdhd");
    buf.extend_from_slice(&[0u8; 4]); // version + flags
    buf.extend_from_slice(&[0u8; 4]); // creation_time
    buf.extend_from_slice(&[0u8; 4]); // modification_time
    buf.extend_from_slice(&timescale.to_be_bytes());
    buf.extend_from_slice(&duration.to_be_bytes());
    buf.extend_from_slice(&0x55C4u16.to_be_bytes()); // language (und)
    buf.extend_from_slice(&0u16.to_be_bytes()); // pre_defined
    buf
}

fn build_hdlr_video() -> Vec<u8> {
    let name = b"VideoHandler\0";
    let size = 33u32 + name.len() as u32;
    let mut buf = Vec::new();
    buf.extend_from_slice(&size.to_be_bytes());
    buf.extend_from_slice(b"hdlr");
    buf.extend_from_slice(&[0u8; 4]); // version + flags
    buf.extend_from_slice(&[0u8; 4]); // pre_defined
    buf.extend_from_slice(b"vide"); // handler_type
    buf.extend_from_slice(&[0u8; 12]); // reserved
    buf.extend_from_slice(name);
    buf
}

fn build_vmhd() -> Vec<u8> {
    let mut buf = Vec::new();
    let size = 20u32;
    buf.extend_from_slice(&size.to_be_bytes());
    buf.extend_from_slice(b"vmhd");
    buf.extend_from_slice(&[0, 0, 0, 1]); // version=0, flags=1
    buf.extend_from_slice(&[0u8; 8]); // graphicsmode + opcolor
    buf
}

fn build_dinf() -> Vec<u8> {
    // dinf -> dref -> url (self-contained)
    let url_box = {
        let mut buf = Vec::new();
        let size = 12u32;
        buf.extend_from_slice(&size.to_be_bytes());
        buf.extend_from_slice(b"url ");
        buf.extend_from_slice(&[0, 0, 0, 1]); // flags=self_contained
        buf
    };
    let dref_box = {
        let mut buf = Vec::new();
        let size = 8u32 + 4 + 4 + url_box.len() as u32;
        buf.extend_from_slice(&size.to_be_bytes());
        buf.extend_from_slice(b"dref");
        buf.extend_from_slice(&[0u8; 4]); // version + flags
        buf.extend_from_slice(&1u32.to_be_bytes()); // entry_count
        buf.extend_from_slice(&url_box);
        buf
    };
    wrap_box(b"dinf", &[&dref_box])
}

fn build_avcc(sps: &[u8], pps: &[u8]) -> Vec<u8> {
    let mut buf = Vec::new();
    buf.push(1); // configurationVersion
    buf.push(sps.get(1).copied().unwrap_or(66)); // AVCProfileIndication
    buf.push(sps.get(2).copied().unwrap_or(0));  // profile_compatibility
    buf.push(sps.get(3).copied().unwrap_or(30)); // AVCLevelIndication
    buf.push(0xFF); // lengthSizeMinusOne = 3 (4-byte NAL length)
    buf.push(0xE1); // numOfSequenceParameterSets = 1
    buf.extend_from_slice(&(sps.len() as u16).to_be_bytes());
    buf.extend_from_slice(sps);
    buf.push(1); // numOfPictureParameterSets
    buf.extend_from_slice(&(pps.len() as u16).to_be_bytes());
    buf.extend_from_slice(pps);
    buf
}

fn build_stsd_avc1(width: u32, height: u32, avcc_data: &[u8]) -> Vec<u8> {
    // avcC box
    let avcc_box_size = 8u32 + avcc_data.len() as u32;
    let mut avcc_box = Vec::new();
    avcc_box.extend_from_slice(&avcc_box_size.to_be_bytes());
    avcc_box.extend_from_slice(b"avcC");
    avcc_box.extend_from_slice(avcc_data);

    // avc1 sample entry
    let avc1_size = 86u32 + avcc_box.len() as u32;
    let mut avc1 = Vec::new();
    avc1.extend_from_slice(&avc1_size.to_be_bytes());
    avc1.extend_from_slice(b"avc1");
    avc1.extend_from_slice(&[0u8; 6]); // reserved
    avc1.extend_from_slice(&1u16.to_be_bytes()); // data_reference_index
    avc1.extend_from_slice(&[0u8; 16]); // pre_defined + reserved
    avc1.extend_from_slice(&(width as u16).to_be_bytes());
    avc1.extend_from_slice(&(height as u16).to_be_bytes());
    avc1.extend_from_slice(&0x00480000u32.to_be_bytes()); // horizresolution = 72 dpi
    avc1.extend_from_slice(&0x00480000u32.to_be_bytes()); // vertresolution = 72 dpi
    avc1.extend_from_slice(&[0u8; 4]); // reserved
    avc1.extend_from_slice(&1u16.to_be_bytes()); // frame_count
    avc1.extend_from_slice(&[0u8; 32]); // compressorname
    avc1.extend_from_slice(&0x0018u16.to_be_bytes()); // depth = 24
    avc1.extend_from_slice(&0xFFFFu16.to_be_bytes()); // pre_defined = -1
    avc1.extend_from_slice(&avcc_box);

    // stsd
    let stsd_size = 16u32 + avc1.len() as u32;
    let mut stsd = Vec::new();
    stsd.extend_from_slice(&stsd_size.to_be_bytes());
    stsd.extend_from_slice(b"stsd");
    stsd.extend_from_slice(&[0u8; 4]); // version + flags
    stsd.extend_from_slice(&1u32.to_be_bytes()); // entry_count
    stsd.extend_from_slice(&avc1);
    stsd
}

fn build_stts(frame_count: u32, sample_duration: u32) -> Vec<u8> {
    let mut buf = Vec::new();
    let size = 24u32;
    buf.extend_from_slice(&size.to_be_bytes());
    buf.extend_from_slice(b"stts");
    buf.extend_from_slice(&[0u8; 4]); // version + flags
    buf.extend_from_slice(&1u32.to_be_bytes()); // entry_count
    buf.extend_from_slice(&frame_count.to_be_bytes());
    buf.extend_from_slice(&sample_duration.to_be_bytes());
    buf
}

fn build_stsc(frame_count: u32) -> Vec<u8> {
    // All samples in one chunk
    let mut buf = Vec::new();
    let size = 28u32;
    buf.extend_from_slice(&size.to_be_bytes());
    buf.extend_from_slice(b"stsc");
    buf.extend_from_slice(&[0u8; 4]); // version + flags
    buf.extend_from_slice(&1u32.to_be_bytes()); // entry_count
    buf.extend_from_slice(&1u32.to_be_bytes()); // first_chunk
    buf.extend_from_slice(&frame_count.to_be_bytes()); // samples_per_chunk
    buf.extend_from_slice(&1u32.to_be_bytes()); // sample_description_index
    buf
}

fn build_stsz(sizes: &[u32]) -> Vec<u8> {
    let mut buf = Vec::new();
    let size = 20u32 + (sizes.len() as u32 * 4);
    buf.extend_from_slice(&size.to_be_bytes());
    buf.extend_from_slice(b"stsz");
    buf.extend_from_slice(&[0u8; 4]); // version + flags
    buf.extend_from_slice(&0u32.to_be_bytes()); // sample_size (0 = variable)
    buf.extend_from_slice(&(sizes.len() as u32).to_be_bytes());
    for &s in sizes {
        buf.extend_from_slice(&s.to_be_bytes());
    }
    buf
}

fn build_stco(offset: u32) -> Vec<u8> {
    // Single chunk at the given offset
    let mut buf = Vec::new();
    let size = 20u32;
    buf.extend_from_slice(&size.to_be_bytes());
    buf.extend_from_slice(b"stco");
    buf.extend_from_slice(&[0u8; 4]); // version + flags
    buf.extend_from_slice(&1u32.to_be_bytes()); // entry_count
    buf.extend_from_slice(&offset.to_be_bytes());
    buf
}

fn build_stss(sync_samples: &[u32]) -> Vec<u8> {
    let mut buf = Vec::new();
    let size = 16u32 + (sync_samples.len() as u32 * 4);
    buf.extend_from_slice(&size.to_be_bytes());
    buf.extend_from_slice(b"stss");
    buf.extend_from_slice(&[0u8; 4]); // version + flags
    buf.extend_from_slice(&(sync_samples.len() as u32).to_be_bytes());
    for &s in sync_samples {
        buf.extend_from_slice(&s.to_be_bytes());
    }
    buf
}

/// Split Annex B byte stream into individual NAL units (without start codes).
fn split_annex_b(data: &[u8]) -> Vec<&[u8]> {
    let mut nals = Vec::new();
    let mut i = 0;

    while i < data.len() {
        // Find start code (0x00 0x00 0x01 or 0x00 0x00 0x00 0x01)
        if i + 2 < data.len() && data[i] == 0 && data[i + 1] == 0 {
            if data[i + 2] == 1 {
                i += 3;
            } else if i + 3 < data.len() && data[i + 2] == 0 && data[i + 3] == 1 {
                i += 4;
            } else {
                i += 1;
                continue;
            }

            // Find the end of this NAL (next start code or end of data)
            let start = i;
            while i < data.len() {
                if i + 2 < data.len() && data[i] == 0 && data[i + 1] == 0
                    && (data[i + 2] == 1 || (i + 3 < data.len() && data[i + 2] == 0 && data[i + 3] == 1))
                {
                    break;
                }
                i += 1;
            }
            if start < i {
                nals.push(&data[start..i]);
            }
        } else {
            i += 1;
        }
    }

    nals
}

fn io_err(e: std::io::Error) -> AppError {
    AppError::Recording(format!("MP4 write failed: {e}"))
}
