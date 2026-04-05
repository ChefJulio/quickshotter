# Phase 2: SCStream Recording Migration

## Overview

Replace the `xcap::Monitor::capture_image()` polling loop in `recording/pipeline.rs` with ScreenCaptureKit's `SCStream` for continuous frame capture on macOS. This is the real performance win — hardware-accelerated frame delivery at the requested FPS with ~50% less CPU usage.

## Current Architecture

### Files
- `src-tauri/src/recording/pipeline.rs` — Main capture + encode pipeline
- `src-tauri/src/recording/encoder.rs` — Platform encoder abstraction  
- `src-tauri/src/recording/encoder_mac.rs` — AVAssetWriter GPU encoder (FFI to avwriter.m)
- `src-tauri/src/recording/avwriter.m` — Objective-C AVAssetWriter implementation
- `src-tauri/src/recording/encoder_cpu.rs` — openh264 fallback
- `src-tauri/src/recording/mp4_muxer.rs` — MP4 container writer for CPU fallback
- `src-tauri/src/sccapture_mac.rs` — ScreenCaptureKit bindings (Phase 1, screenshots only)

### Current Flow (macOS)
```
capture_loop() or capture_loop_multi()
  ├─ Runs on a dedicated thread
  ├─ Loop:
  │   ├─ Sleep to pace at target FPS
  │   ├─ xcap::Monitor::capture_image()  ← ~30-50ms per frame, CPU-bound
  │   ├─ Detect Retina scale on first frame
  │   ├─ Crop to region if specified
  │   └─ Send TimestampedFrame to encoder via bounded channel (4-frame buffer)
  └─ Encoder thread receives frames and encodes to MP4/GIF
```

### Problems with Current Approach
1. **CPU polling**: `capture_image()` is called in a tight loop with `thread::sleep` for pacing — wastes CPU cycles
2. **Frame timing**: Manual FPS pacing with `Instant::now()` + sleep is imprecise, causes frame drops
3. **Retina detection**: Hacky first-frame comparison to detect scale factor
4. **No GPU acceleration**: xcap captures via CGWindowListCreateImage (deprecated, CPU-bound)

## Target Architecture

### SCStream Flow
```
SCStream (configured at target FPS)
  ├─ macOS delivers frames via SCStreamOutput delegate
  ├─ Frames arrive as CMSampleBuffer with IOSurface backing (GPU memory)
  ├─ Convert to RGBA in callback, enqueue to channel
  └─ Encoder thread unchanged — still receives TimestampedFrame
```

### Key Advantages
- **Hardware-accelerated**: Frames composited by WindowServer GPU, not CPU
- **Precise FPS**: macOS delivers frames at exactly the requested rate
- **No polling**: Event-driven, zero CPU between frames
- **Region cropping on GPU**: `sourceRect` on SCStreamConfiguration handles crop
- **No Retina hacks**: Control output resolution explicitly via `width`/`height`

## Implementation Plan

### Step 1: Add SCStream wrapper to `sccapture_mac.rs`

The `screencapturekit` crate already has SCStream support. Add functions:

```rust
// In sccapture_mac.rs:

use screencapturekit::stream::SCStream;
use screencapturekit::stream::output::SCStreamOutputHandler;

pub struct StreamHandle {
    stream: SCStream,
    receiver: crossbeam_channel::Receiver<TimestampedFrame>,
}

/// Start a capture stream for recording.
/// Returns a handle with a receiver that delivers frames at the target FPS.
pub fn start_stream(
    display_id: u32,
    region: Option<(f64, f64, f64, f64)>,  // x, y, w, h in logical points
    fps: u32,
    output_width: u32,
    output_height: u32,
) -> Result<StreamHandle, AppError> {
    // 1. Get cached display from ensure_cache()
    // 2. Create SCContentFilter for the display
    // 3. Create SCStreamConfiguration with:
    //    - fps via with_minimum_frame_interval or with_fps
    //    - width/height for output resolution
    //    - source_rect for region cropping (if specified)
    //    - pixel_format BGRA
    //    - shows_cursor false
    // 4. Create SCStream with filter + config
    // 5. Set up output handler that:
    //    - Receives CMSampleBuffer frames
    //    - Extracts RGBA pixel data
    //    - Sends TimestampedFrame to channel
    // 6. Start the stream
    // 7. Return handle with receiver
}

/// Stop and clean up the stream.
pub fn stop_stream(handle: StreamHandle) {
    // stream.stop() + drop
}
```

**Key question**: Does the `screencapturekit` crate's `SCStream` API support receiving frames synchronously? Check:
- `screencapturekit::stream::SCStream` — look for `add_output` or `SCStreamOutputHandler` trait
- The crate may have a callback-based API or a polling-based one
- If callback-based, the handler sends frames to a crossbeam channel
- If the crate has an `AsyncSCStream`, that could also work with tokio

### Step 2: Add `capture_loop_scstream()` to `pipeline.rs`

New function that replaces `capture_loop()` on macOS:

```rust
#[cfg(target_os = "macos")]
fn capture_loop_scstream(
    display_id: u32,
    fps: u32,
    region: Option<RecordingRegion>,
    stop_signal: Arc<AtomicBool>,
    sender: Sender<TimestampedFrame>,
    format: RecordingFormat,
    max_duration_secs: u32,
) {
    // 1. Calculate output dimensions (region size * Retina scale, or full display)
    // 2. Call sccapture_mac::start_stream(display_id, region, fps, w, h)
    // 3. Loop:
    //    - Receive frames from stream handle's receiver
    //    - Check stop_signal
    //    - Check max_duration for GIF
    //    - Forward TimestampedFrame to encoder sender
    // 4. On stop: call sccapture_mac::stop_stream()
}
```

### Step 3: Update `start_pipeline()` to use SCStream on macOS

In `start_pipeline()`, replace the `capture_loop` thread spawn:

```rust
#[cfg(target_os = "macos")]
let capture_thread = {
    // Use SCStream instead of xcap polling
    let display_id = /* get from cached displays */;
    std::thread::Builder::new()
        .name("recording-capture".into())
        .spawn(move || {
            capture_loop_scstream(display_id, capture_fps, capture_region, capture_stop, sender, format, max_duration);
        })?
};

#[cfg(not(target_os = "macos"))]
let capture_thread = {
    // Windows: keep existing xcap/BitBlt loop
    // ... existing code ...
};
```

### Step 4: Handle multi-monitor recording

Current `capture_loop_multi()` captures from multiple monitors and stitches frames. With SCStream:

**Option A (simple)**: Create one SCStream per monitor, receive frames from each, stitch in Rust. Same approach, just faster frame delivery.

**Option B (better)**: Use a single SCStream with `SCContentFilter` that includes all displays. SCStream can capture the full virtual desktop in one stream. Set `sourceRect` to the recording region that spans monitors.

Recommend Option A initially for simplicity, migrate to B if performance warrants it.

### Step 5: Frame format handling

SCStream delivers `CMSampleBuffer` frames. The `screencapturekit` crate likely exposes these as:
- Raw pixel data (BGRA format)
- Or via `CGImage` conversion

The pixel data needs to be:
1. Converted from BGRA to RGBA (or configure SCStream for RGBA if supported)
2. Wrapped in `RgbaImage` 
3. Sent as `TimestampedFrame` with the CMSampleBuffer's presentation timestamp

**Check**: The crate's `SCStreamOutputHandler` trait — what format does it deliver frames in? Does it already handle BGRA→RGBA?

### Step 6: Timestamp handling

Current approach: `Instant::now() - start_time` for each frame's PTS.

With SCStream: Each `CMSampleBuffer` has a precise `CMSampleTimingInfo` with `presentationTimeStamp`. Use this instead — it's more accurate than our manual timing.

### Step 7: Remove xcap from macOS recording path

After SCStream is working:
- Remove `xcap::Monitor` usage from `capture_loop()` and `capture_loop_multi()`
- Gate the xcap-based loops with `#[cfg(not(target_os = "macos"))]`
- Keep xcap for Windows (BitBlt) and the `window_capture.rs` detection

## Testing Plan

1. **Region recording**: Select a region, record 5 seconds MP4, verify correct region + smooth playback
2. **Fullscreen recording**: Record primary monitor, verify no artifacts
3. **GIF recording**: Region GIF with max duration, verify auto-stop
4. **Retina**: Verify output resolution is 2x on Retina displays
5. **FPS**: Record at 30fps, verify playback is smooth (no frame drops)
6. **Compare CPU usage**: Activity Monitor during xcap recording vs SCStream recording — should see ~50% reduction

## Risks

1. **`screencapturekit` crate SCStream API**: May not expose a convenient frame-receiving API. Check the crate's `stream` module thoroughly. If it only has async APIs, may need tokio integration or a manual CFRunLoop.

2. **Frame delivery latency**: SCStream has ~100-200ms startup latency. First frames may be delayed. Handle by waiting for first frame before starting the encoder timer.

3. **CMSampleBuffer pixel format**: May be in a format the encoder doesn't expect. The AVAssetWriter encoder already handles BGRA (it was designed for CoreVideo buffers), so this might actually simplify the pipeline — pass the CVPixelBuffer directly to the encoder without RGBA conversion.

4. **Thread safety**: SCStream callbacks come on a dispatch queue. The crossbeam channel sender must be `Send`. Wrap appropriately.

## Files to Modify

- `src-tauri/src/sccapture_mac.rs` — Add SCStream wrapper functions
- `src-tauri/src/recording/pipeline.rs` — Add `capture_loop_scstream()`, gate xcap loops
- `src-tauri/src/recording/mod.rs` — May need to expose new types
- `src-tauri/src/commands.rs` — Update `start_region_recording` to pass display_id

## Dependencies

- `screencapturekit` crate already added (v1.5, `macos_14_0` feature)
- No new dependencies needed
- Windows path completely unchanged

## Estimated Complexity

Medium-high. The main unknown is the `screencapturekit` crate's SCStream API ergonomics. If it has a clean frame-receiving API, this is ~200 lines of new Rust code. If it requires manual CFRunLoop management, it's more complex.

Start by reading the crate's `stream` module source code and examples before writing any code.
