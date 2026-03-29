use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, Sender, bounded};
use image::RgbaImage;
use xcap::Monitor;

use crate::error::AppError;

/// Wrapper to make `xcap::Monitor` Send-safe.
/// `HMONITOR` (Windows) is a process-global handle, not a thread-local resource,
/// so it's safe to use across threads despite containing a raw pointer.
struct SendMonitor(Monitor);
unsafe impl Send for SendMonitor {}

/// Region to record (xcap coordinate space).
#[derive(Debug, Clone)]
pub struct RecordingRegion {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

/// Format to record in.
#[derive(Debug, Clone, PartialEq)]
pub enum RecordingFormat {
    Mp4,
    Gif,
}

/// A captured frame with its presentation timestamp.
pub struct TimestampedFrame {
    pub image: RgbaImage,
    pub pts_ms: f64,
}

/// How to capture frames for recording.
#[derive(Debug, Clone)]
pub enum CaptureSource {
    /// Capture from a single monitor (by index into Monitor::all()).
    /// Region coords are monitor-local.
    SingleMonitor(usize),
    /// Capture from all monitors (stitched). Region coords are
    /// overlay-relative (= desktop-origin-relative physical pixels).
    FullDesktop,
}

/// Monitor info gathered before spawning capture thread.
struct MonitorInfo {
    monitor: SendMonitor,
    x: i32,
    y: i32,
    width: u32,
    height: u32,
}

/// Pre-computed overlap between a monitor and the recording region.
struct OverlapRect {
    monitor_idx: usize,
    src_x: u32, src_y: u32, src_w: u32, src_h: u32,
    dst_x: u32, dst_y: u32,
}

/// Recording pipeline configuration.
pub struct PipelineConfig {
    pub format: RecordingFormat,
    pub fps: u32,
    pub region: Option<RecordingRegion>,
    pub output_path: PathBuf,
    pub gif_max_duration_secs: u32,
    pub gif_max_width: u32,
    pub capture_source: CaptureSource,
}

/// Handle to a running recording pipeline.
pub struct PipelineHandle {
    pub stop_signal: Arc<AtomicBool>,
    capture_thread: Option<JoinHandle<()>>,
    encoder_thread: Option<JoinHandle<Result<PathBuf, AppError>>>,
}

impl PipelineHandle {
    /// Signal the pipeline to stop and wait for both threads to finish.
    /// Returns the path to the output file on success.
    pub fn stop_and_wait(mut self) -> Result<PathBuf, AppError> {
        self.stop_signal.store(true, Ordering::Relaxed);

        // Wait for capture thread to finish (it exits when stop_signal is set)
        if let Some(handle) = self.capture_thread.take() {
            handle.join().ok();
        }

        // Wait for encoder thread to finish and get the result
        if let Some(handle) = self.encoder_thread.take() {
            match handle.join() {
                Ok(result) => return result,
                Err(_) => return Err(AppError::Recording("Encoder thread panicked".to_string())),
            }
        }

        Err(AppError::Recording("No encoder thread".to_string()))
    }
}

/// Start the recording pipeline.
/// Spawns a capture thread and an encoder thread connected by a bounded channel.
pub fn start_pipeline(config: PipelineConfig) -> Result<PipelineHandle, AppError> {
    let stop_signal = Arc::new(AtomicBool::new(false));

    // Bounded channel: 4 frames of buffer.
    // If encoder can't keep up, capture thread drops frames.
    let (sender, receiver): (Sender<TimestampedFrame>, Receiver<TimestampedFrame>) = bounded(4);

    let capture_stop = stop_signal.clone();
    let capture_region = config.region.clone();
    let capture_fps = config.fps;
    let format = config.format.clone();
    let max_duration = config.gif_max_duration_secs;

    let capture_thread = match config.capture_source {
        CaptureSource::SingleMonitor(idx) => {
            let monitors = Monitor::all().map_err(|e| AppError::Recording(format!("Failed to enumerate monitors: {e}")))?;
            let monitor = monitors.into_iter().nth(idx)
                .ok_or_else(|| AppError::Recording(format!("Monitor index {} not found", idx)))?;
            let send_monitor = SendMonitor(monitor);

            std::thread::Builder::new()
                .name("recording-capture".into())
                .spawn(move || {
                    capture_loop(send_monitor, capture_fps, capture_region, capture_stop, sender, format, max_duration);
                })
                .map_err(|e| AppError::Recording(format!("Failed to spawn capture thread: {e}")))?
        }
        CaptureSource::FullDesktop => {
            let monitors = Monitor::all().map_err(|e| AppError::Recording(format!("Failed to enumerate monitors: {e}")))?;
            let mut infos = Vec::new();
            for m in monitors {
                let x = m.x().unwrap_or(0);
                let y = m.y().unwrap_or(0);
                let w = m.width().unwrap_or(0);
                let h = m.height().unwrap_or(0);
                infos.push(MonitorInfo { monitor: SendMonitor(m), x, y, width: w, height: h });
            }
            if infos.is_empty() {
                return Err(AppError::Recording("No monitors found".to_string()));
            }

            std::thread::Builder::new()
                .name("recording-capture".into())
                .spawn(move || {
                    capture_loop_multi(infos, capture_fps, capture_region, capture_stop, sender, format, max_duration);
                })
                .map_err(|e| AppError::Recording(format!("Failed to spawn capture thread: {e}")))?
        }
    };

    let output_path = config.output_path.clone();
    let encoder_format = config.format.clone();
    let encoder_fps = config.fps;
    let gif_max_width = config.gif_max_width;

    let encoder_thread = std::thread::Builder::new()
        .name("recording-encoder".into())
        .spawn(move || {
            encode_loop(receiver, encoder_format, encoder_fps, output_path, gif_max_width)
        })
        .map_err(|e| AppError::Recording(format!("Failed to spawn encoder thread: {e}")))?;

    Ok(PipelineHandle {
        stop_signal,
        capture_thread: Some(capture_thread),
        encoder_thread: Some(encoder_thread),
    })
}

fn capture_loop(
    monitor: SendMonitor,
    fps: u32,
    region: Option<RecordingRegion>,
    stop_signal: Arc<AtomicBool>,
    sender: Sender<TimestampedFrame>,
    format: RecordingFormat,
    max_duration_secs: u32,
) {
    let frame_interval = Duration::from_secs_f64(1.0 / fps as f64);
    let start_time = Instant::now();
    let mut next_frame_time = start_time;
    let mut logged_first_frame = false;
    let mut sent_count: u32 = 0;
    let mut dropped_count: u32 = 0;

    // On macOS Retina, capture_image() returns physical pixels (2x) but
    // region coords are in xcap's coordinate space (logical points).
    // Computed on first frame and cached for the rest of the recording.
    let mut scaled_region: Option<RecordingRegion> = None;

    eprintln!("recording: capture_loop started (single-monitor, {}fps)", fps);

    loop {
        if stop_signal.load(Ordering::Relaxed) {
            break;
        }

        // Auto-stop GIF recordings after max duration
        if format == RecordingFormat::Gif && max_duration_secs > 0 {
            if start_time.elapsed().as_secs() >= max_duration_secs as u64 {
                stop_signal.store(true, Ordering::Relaxed);
                break;
            }
        }

        // Pace at target FPS
        let now = Instant::now();
        if now < next_frame_time {
            std::thread::sleep(next_frame_time - now);
        }
        next_frame_time += frame_interval;

        // Capture
        let img = match monitor.0.capture_image() {
            Ok(img) => img,
            Err(e) => {
                eprintln!("recording: capture failed: {e}");
                continue;
            }
        };

        // On first frame, detect Retina/HiDPI scaling and compute scaled crop region.
        // Region coords are in xcap space (logical on macOS), but capture_image()
        // returns physical pixels. Scale region to match actual capture resolution.
        if !logged_first_frame {
            if let Some(ref r) = region {
                let mon_w = monitor.0.width().unwrap_or(img.width());
                let mon_h = monitor.0.height().unwrap_or(img.height());
                let sx = img.width() as f64 / mon_w as f64;
                let sy = img.height() as f64 / mon_h as f64;
                eprintln!("recording: first frame capture={}x{} monitor={}x{} scale={:.2}x{:.2} region=({},{} {}x{})",
                    img.width(), img.height(), mon_w, mon_h, sx, sy, r.x, r.y, r.width, r.height);
                if (sx - 1.0).abs() > 0.01 || (sy - 1.0).abs() > 0.01 {
                    scaled_region = Some(RecordingRegion {
                        x: (r.x as f64 * sx) as u32,
                        y: (r.y as f64 * sy) as u32,
                        width: (r.width as f64 * sx) as u32,
                        height: (r.height as f64 * sy) as u32,
                    });
                    let sr = scaled_region.as_ref().unwrap();
                    eprintln!("recording: scaled region -> ({},{} {}x{})", sr.x, sr.y, sr.width, sr.height);
                }
            } else {
                eprintln!("recording: first frame capture={}x{} (fullscreen)", img.width(), img.height());
            }
            logged_first_frame = true;
        }

        // Crop to region if specified (use scaled region for Retina displays)
        let frame = if region.is_some() {
            let r = scaled_region.as_ref().or(region.as_ref()).unwrap();
            let x = r.x.min(img.width().saturating_sub(1));
            let y = r.y.min(img.height().saturating_sub(1));
            let w = r.width.min(img.width().saturating_sub(x));
            let h = r.height.min(img.height().saturating_sub(y));
            if w == 0 || h == 0 { continue; }
            image::imageops::crop_imm(&img, x, y, w, h).to_image()
        } else {
            img
        };

        let pts_ms = start_time.elapsed().as_secs_f64() * 1000.0;

        // Send to encoder. If the channel is full, drop this frame.
        match sender.try_send(TimestampedFrame { image: frame, pts_ms }) {
            Ok(()) => sent_count += 1,
            Err(_) => dropped_count += 1,
        }
    }

    eprintln!("recording: capture_loop stopped (sent={} dropped={})", sent_count, dropped_count);

    // Drop sender to signal the encoder that no more frames are coming
    drop(sender);
}

/// Multi-monitor capture loop. Captures all monitors that overlap with the
/// region and composites them into a region-sized output canvas each frame.
fn capture_loop_multi(
    monitors: Vec<MonitorInfo>,
    fps: u32,
    region: Option<RecordingRegion>,
    stop_signal: Arc<AtomicBool>,
    sender: Sender<TimestampedFrame>,
    format: RecordingFormat,
    max_duration_secs: u32,
) {
    let frame_interval = Duration::from_secs_f64(1.0 / fps as f64);
    let start_time = Instant::now();
    let mut next_frame_time = start_time;
    let mut logged_first_frame = false;
    let mut sent_count: u32 = 0;
    let mut dropped_count: u32 = 0;

    eprintln!("recording: capture_loop_multi started ({} monitors, {}fps)", monitors.len(), fps);

    // Desktop origin (top-left of virtual desktop)
    let desk_x = monitors.iter().map(|m| m.x).min().unwrap_or(0);
    let desk_y = monitors.iter().map(|m| m.y).min().unwrap_or(0);

    // Pre-compute overlaps between each monitor and the region.
    // Region coords are overlay-relative = desktop-origin-relative.
    let overlaps: Vec<OverlapRect> = if let Some(ref r) = region {
        let reg_abs_x = r.x as i32 + desk_x;
        let reg_abs_y = r.y as i32 + desk_y;
        let reg_abs_r = reg_abs_x + r.width as i32;
        let reg_abs_b = reg_abs_y + r.height as i32;

        eprintln!("recording: multi region=({},{} {}x{}) desk_origin=({},{}) abs=({},{} -> {},{})",
            r.x, r.y, r.width, r.height, desk_x, desk_y, reg_abs_x, reg_abs_y, reg_abs_r, reg_abs_b);

        monitors.iter().enumerate().filter_map(|(i, m)| {
            let ol = reg_abs_x.max(m.x);
            let ot = reg_abs_y.max(m.y);
            let or_ = reg_abs_r.min(m.x + m.width as i32);
            let ob = reg_abs_b.min(m.y + m.height as i32);
            eprintln!("recording: multi monitor[{}] pos=({},{}) size={}x{} overlap=({},{} -> {},{}) valid={}",
                i, m.x, m.y, m.width, m.height, ol, ot, or_, ob, or_ > ol && ob > ot);
            if or_ > ol && ob > ot {
                Some(OverlapRect {
                    monitor_idx: i,
                    src_x: (ol - m.x) as u32,
                    src_y: (ot - m.y) as u32,
                    src_w: (or_ - ol) as u32,
                    src_h: (ob - ot) as u32,
                    dst_x: (ol - reg_abs_x) as u32,
                    dst_y: (ot - reg_abs_y) as u32,
                })
            } else {
                None
            }
        }).collect()
    } else {
        Vec::new()
    };

    eprintln!("recording: multi {} overlaps, output={}x{}",
        overlaps.len(), if let Some(ref r) = region { r.width } else { 0 },
        if let Some(ref r) = region { r.height } else { 0 });

    // Output dimensions
    let (out_w, out_h) = if let Some(ref r) = region {
        (r.width, r.height)
    } else {
        let dx = monitors.iter().map(|m| m.x + m.width as i32).max().unwrap_or(0);
        let dy = monitors.iter().map(|m| m.y + m.height as i32).max().unwrap_or(0);
        ((dx - desk_x) as u32, (dy - desk_y) as u32)
    };

    loop {
        if stop_signal.load(Ordering::Relaxed) { break; }

        if format == RecordingFormat::Gif && max_duration_secs > 0 {
            if start_time.elapsed().as_secs() >= max_duration_secs as u64 {
                stop_signal.store(true, Ordering::Relaxed);
                break;
            }
        }

        let now = Instant::now();
        if now < next_frame_time {
            std::thread::sleep(next_frame_time - now);
        }
        next_frame_time += frame_interval;

        let mut canvas = RgbaImage::new(out_w, out_h);

        if region.is_some() && !overlaps.is_empty() {
            // Capture only monitors that overlap, copy only the overlapping strip
            let mut ok = true;
            for ov in &overlaps {
                let img = match monitors[ov.monitor_idx].monitor.0.capture_image() {
                    Ok(img) => img,
                    Err(e) => { eprintln!("recording: multi capture failed: {e}"); ok = false; break; }
                };
                if !logged_first_frame {
                    eprintln!("recording: multi-mon monitor[{}] capture={}x{} overlap src=({},{} {}x{}) dst=({},{})",
                        ov.monitor_idx, img.width(), img.height(),
                        ov.src_x, ov.src_y, ov.src_w, ov.src_h, ov.dst_x, ov.dst_y);
                }
                // Copy overlap strip from monitor image to canvas (row-wise bulk copy)
                let sw = ov.src_w.min(img.width().saturating_sub(ov.src_x));
                let sh = ov.src_h.min(img.height().saturating_sub(ov.src_y));
                let copy_h = sh.min(out_h.saturating_sub(ov.dst_y));
                let copy_w = sw.min(out_w.saturating_sub(ov.dst_x));
                let src_stride = img.width() as usize * 4;
                let dst_stride = out_w as usize * 4;
                let src_buf = img.as_raw();
                let dst_buf = canvas.as_mut();
                for py in 0..copy_h {
                    let src_off = (ov.src_y + py) as usize * src_stride + ov.src_x as usize * 4;
                    let dst_off = (ov.dst_y + py) as usize * dst_stride + ov.dst_x as usize * 4;
                    let len = copy_w as usize * 4;
                    dst_buf[dst_off..dst_off + len].copy_from_slice(&src_buf[src_off..src_off + len]);
                }
            }
            if !logged_first_frame {
                eprintln!("recording: multi-mon output={}x{}", out_w, out_h);
                logged_first_frame = true;
            }
            if !ok { continue; }
        } else {
            // No region: stitch full desktop (fullscreen multi-monitor recording)
            let mut ok = true;
            for info in &monitors {
                let img = match info.monitor.0.capture_image() {
                    Ok(img) => img,
                    Err(e) => { eprintln!("recording: multi capture failed: {e}"); ok = false; break; }
                };
                let ox = (info.x - desk_x) as u32;
                let oy = (info.y - desk_y) as u32;
                let copy_h = img.height().min(out_h.saturating_sub(oy));
                let copy_w = img.width().min(out_w.saturating_sub(ox));
                let src_stride = img.width() as usize * 4;
                let dst_stride = out_w as usize * 4;
                let src_buf = img.as_raw();
                let dst_buf = canvas.as_mut();
                for py in 0..copy_h {
                    let src_off = py as usize * src_stride;
                    let dst_off = (oy + py) as usize * dst_stride + ox as usize * 4;
                    let len = copy_w as usize * 4;
                    dst_buf[dst_off..dst_off + len].copy_from_slice(&src_buf[src_off..src_off + len]);
                }
            }
            if !ok { continue; }
        }

        let pts_ms = start_time.elapsed().as_secs_f64() * 1000.0;
        match sender.try_send(TimestampedFrame { image: canvas, pts_ms }) {
            Ok(()) => sent_count += 1,
            Err(_) => dropped_count += 1,
        }
    }

    eprintln!("recording: capture_loop_multi stopped (sent={} dropped={})", sent_count, dropped_count);
    drop(sender);
}

fn encode_loop(
    receiver: Receiver<TimestampedFrame>,
    format: RecordingFormat,
    fps: u32,
    output_path: PathBuf,
    gif_max_width: u32,
) -> Result<PathBuf, AppError> {
    match format {
        RecordingFormat::Gif => {
            encode_gif(receiver, fps, &output_path, gif_max_width)?;
            Ok(output_path)
        }
        RecordingFormat::Mp4 => {
            encode_mp4(receiver, fps, &output_path)?;
            Ok(output_path)
        }
    }
}

fn encode_gif(
    receiver: Receiver<TimestampedFrame>,
    fps: u32,
    output_path: &std::path::Path,
    max_width: u32,
) -> Result<(), AppError> {
    use super::gif_encoder::{GifConfig, GifEncoder};

    // GIF delay is in centiseconds (1/100s)
    let frame_delay_cs = (100.0 / fps as f64).round() as u16;
    let frame_delay_cs = frame_delay_cs.max(2); // minimum 20ms per GIF spec

    let config = GifConfig {
        max_width,
        frame_delay_cs,
    };

    let mut encoder = GifEncoder::new();
    encoder.init(&config, output_path)?;

    for frame in receiver.iter() {
        let (w, h) = (frame.image.width(), frame.image.height());
        encoder.add_frame(frame.image.as_raw(), w, h, output_path)?;
    }

    encoder.finish()?;
    eprintln!("recording: GIF saved to {:?} ({} frames)", output_path, encoder.frame_count());

    if encoder.frame_count() == 0 {
        return Err(AppError::Recording("No frames captured for GIF".to_string()));
    }

    Ok(())
}

fn encode_mp4(
    receiver: Receiver<TimestampedFrame>,
    fps: u32,
    output_path: &std::path::Path,
) -> Result<(), AppError> {
    use super::encoder::{EncoderConfig, create_encoder};

    let mut encoder = create_encoder();
    let mut initialized = false;
    let mut enc_w: u32 = 0;
    let mut enc_h: u32 = 0;
    let mut frame_count: u32 = 0;

    for frame in receiver.iter() {
        let (w, h) = (frame.image.width(), frame.image.height());

        // Lazy init: we need width/height from the first frame
        if !initialized {
            // H.264 requires even dimensions; round down if odd
            enc_w = w & !1;
            enc_h = h & !1;
            if enc_w == 0 || enc_h == 0 {
                eprintln!("recording: skipping frame with 0 even-aligned dimension: {}x{} -> {}x{}", w, h, enc_w, enc_h);
                continue;
            }
            let bitrate = compute_bitrate(enc_w, enc_h, fps);
            let config = EncoderConfig {
                width: enc_w,
                height: enc_h,
                fps,
                bitrate,
                output_path: output_path.to_path_buf(),
            };
            eprintln!("recording: init encoder {}x{} @ {}fps bitrate={} path={:?}", enc_w, enc_h, fps, bitrate, output_path);
            encoder.init(&config)?;
            eprintln!("recording: encoder initialized: {}", encoder.name());
            initialized = true;
        }

        // Crop to even dimensions if needed (H.264 requirement)
        let frame_img = if w != enc_w || h != enc_h {
            image::imageops::crop_imm(&frame.image, 0, 0, enc_w, enc_h).to_image()
        } else {
            frame.image
        };
        encoder.encode_frame(frame_img.as_raw(), enc_w, enc_h, frame.pts_ms)?;
        frame_count += 1;
        if frame_count <= 3 || frame_count % 30 == 0 {
            eprintln!("recording: encoded frame {} pts={:.1}ms", frame_count, frame.pts_ms);
        }
    }

    eprintln!("recording: channel closed, total frames received: {}", frame_count);

    if !initialized {
        return Err(AppError::Recording("No frames captured for MP4".to_string()));
    }

    eprintln!("recording: calling encoder.finish()...");
    encoder.finish()?;

    // Verify the file was actually written
    let file_size = std::fs::metadata(output_path).map(|m| m.len()).unwrap_or(0);
    eprintln!("recording: MP4 saved to {:?} (size={} bytes, {} frames)", output_path, file_size, frame_count);

    if file_size == 0 {
        return Err(AppError::Recording(format!(
            "Encoder produced empty file after {} frames - encoding may have failed silently", frame_count
        )));
    }

    Ok(())
}

/// Compute a reasonable bitrate based on resolution and FPS.
/// Targets roughly 5 bits per pixel at 30fps, scaled linearly.
fn compute_bitrate(width: u32, height: u32, fps: u32) -> u32 {
    let pixels = width as u64 * height as u64;
    let bpp = 5u64; // bits per pixel
    let bitrate = pixels * bpp * fps as u64 / 30;
    // Clamp between 1 Mbps and 50 Mbps
    bitrate.max(1_000_000).min(50_000_000) as u32
}
