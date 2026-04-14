# Freeze Mode Image Pipeline

## Why does downscaling the overlay texture not degrade capture quality?

On multi-monitor Retina setups, the overlay window's physical pixel dimensions can exceed the GPU's maximum texture size (8192px for most GPUs). When this happens, the overlay daemon downscales its wgpu surface and screenshot texture to fit. This raises a natural question: if freeze mode captures the entire desktop and displays it as a frozen background, doesn't downscaling that image degrade the final capture?

**No.** The final capture is always full resolution. Here's why.

## Two independent copies of the screenshot

When freeze mode activates, the Tauri host captures the full desktop and creates two separate copies:

### 1. `state.pending_screenshot` (in-memory, full resolution)

Stored in `AppState` by `open_overlay_with_mode()` in `src-tauri/src/overlay.rs`:

```
state.pending_screenshot = Some(screen.image);
```

This `RgbaImage` is kept in Tauri's process memory at full Retina resolution. It is never sent to the GPU and never downscaled. When the user finishes their region selection, `complete_region_capture_inner()` in `src-tauri/src/commands.rs` crops directly from this image:

```
let img = safe_crop(screenshot, left, top, w, h)?;
```

This is what produces the user's final capture.

### 2. Temp file for the overlay daemon (may be downscaled)

Written to `/tmp/qs_overlay_capture.raw` by `open_overlay_with_mode()`:

```
std::fs::write(&temp, screen.image.as_raw())
```

The overlay daemon reads this file, optionally downsamples it to fit within the GPU's `max_texture_dimension_2d`, and uploads it as a wgpu texture. This texture serves as the frozen screenshot background behind the semi-transparent selection overlay. It is purely a visual aid for the user while dragging to select a region. It is destroyed when the overlay window closes.

## Data flow diagram

```
capture_all_monitors()
        |
        v
  full-res RgbaImage
       / \
      /   \
     v     v
pending_screenshot          temp file on disk
(Tauri host memory)         (/tmp/qs_overlay_capture.raw)
     |                              |
     |                     overlay daemon reads
     |                              |
     |                     downscale if > 8192px
     |                              |
     |                     GPU texture (visual only)
     |                              |
     |                     user drags selection
     |                              |
     |                     selection coords sent
     |                     back to Tauri host
     |                              |
     v                              v
safe_crop(pending_screenshot, coords)
     |
     v
  final capture (full resolution)
```

## Key takeaway

The GPU texture in the overlay daemon is a disposable visual preview. The actual capture output always comes from the original full-resolution image held in Tauri's process memory. Downscaling the overlay texture for GPU compatibility has zero effect on output quality.
