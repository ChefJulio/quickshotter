# Performance Opportunities

Identified 2025-03-29. Work through each item, check off when done.

---

## Critical (measurable user-facing improvement)

- [x] **1. Window detection busy-wait burns 100% CPU**
  `src-tauri/src/window_capture.rs:49-64` — Added 30ms sleep between polls when active.

- [x] **2. Pixel-by-pixel copy in multi-monitor recording**
  `src-tauri/src/recording/pipeline.rs:394-420` — Replaced `put_pixel`/`get_pixel` nested loops with row-wise `copy_from_slice`. ~10-50x faster for multi-monitor stitching.

- [x] **3. GIF encoder re-allocates entire frame buffer every frame**
  `src-tauri/src/recording/gif_encoder.rs:57-62` — Reuse `rgb_buf` and `index_buf` across frames. Use `Cow::Borrowed` to skip `rgba.to_vec()` when no downscale needed.

- [ ] **4. MP4 muxer stores ALL frames in memory**
  `src-tauri/src/recording/mp4_muxer.rs` — Intentional design: MP4 requires moov box with all frame sizes/offsets. Would need mdat-first layout to stream. Low priority for typical <60s recordings (~45MB).

---

## High (noticeable snappiness improvement)

- [x] **5. OCR clones the entire image when no upscaling needed**
  `src-tauri/src/ocr.rs` — Changed `upscale_if_needed` to return `Cow<RgbaImage>`, avoiding 8MB clone in the common case.

- [x] **6. Dead pending_base64 field wasting ~44MB per freeze capture**
  `src-tauri/src/state.rs` — Removed `pending_base64` field entirely. Also removed dead `get_pending_screenshot` command. Fixed OCR dispatch (was routing to region capture instead of OCR).

- [ ] **7. Live overlay does full-buffer memcpy every frame**
  `overlay/src/live_renderer.rs:214-219` — Copies entire pixel buffer to window surface every frame (~8MB at 1080p, 500MB/s at 60fps). Dirty rectangle tracking would reduce to only changed regions.

- [x] **8. Catbox upload: encode to memory, write to disk, curl reads it back**
  `src-tauri/src/catbox.rs` — Now encodes PNG directly to temp file via `BufWriter` with fast compression. Eliminated intermediate `Vec<u8>` buffer.

---

## Medium (cleanup / future-proofing)

- [x] **9. Overlay window detection polls at 62.5Hz**
  `overlay/src/window_detect.rs:57` — Reduced from 16ms to 50ms interval. 3x less CPU usage.

- [ ] **10. Freeze renderer writes GPU uniform buffer every frame**
  `overlay/src/renderer.rs:172-179` — Even when the selection hasn't changed. Should diff against previous state and skip the write.

- [ ] **11. Tray menu clones full config every rebuild**
  `src-tauri/src/tray.rs:201-206` — `state.config.clone()` (32+ string fields) + history path clones on every capture. Runs on background thread now so it doesn't block, but still wasteful.

- [ ] **12. Startup blocks on config load + registry ops**
  `src-tauri/src/lib.rs:106-119` — `load_config()`, `set_launch_on_startup()`, `set_context_menu()` all do synchronous file/registry I/O before the window appears.

---

## Unconventional ideas

- [ ] **13. Pre-warm the clipboard**
  `Clipboard::new()` is called fresh on every capture. On Windows this opens/closes the clipboard handle. Could keep a thread-local clipboard instance alive.

- [ ] **14. Memory-mapped temp files**
  For the annotation temp JPEG and overlay freeze-mode raw file, use `memmap2` instead of `std::fs::write`. The OS handles paging and the "write" is effectively instant (just dirty pages).

- [ ] **15. Default to JPEG saves instead of PNG**
  A 4K JPEG encodes in ~20ms vs ~150ms for fast PNG, and the file is 5x smaller. PNG is only needed for pixel-perfect screenshots with transparency. Could default to JPEG and let users opt into PNG.

---

## Also done (not originally in list)

- [x] **Fast PNG compression** — Switched all PNG encoding to `CompressionType::Fast` + `FilterType::Sub` (~3-5x faster).
- [x] **Parallel clipboard + disk save** — Clipboard copy is synchronous, disk save fires on background thread.
- [x] **Annotation editor: asset protocol instead of base64** — Writes temp JPEG, loads via `convertFileSrc`. Eliminated ~60MB base64 IPC roundtrip.
- [x] **Compositor wait reduction** — Daemon 50ms -> 34ms, host-side 150ms -> 50ms.
- [x] **Desktop bounds caching** — `Monitor::all()` called once per capture session, cached in `AppState`.
- [x] **Freeze mode: no white flash** — Window starts hidden, first frame renders before `set_visible(true)`.
- [x] **Rename instant -> live** — Config migration from "instant" to "live" with backwards compatibility.
