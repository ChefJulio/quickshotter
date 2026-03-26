# QuickShotter

A lightweight screenshot and screen recording tool for Windows and macOS, built with Tauri 2, Rust, and TypeScript. Lives in your system tray with no persistent window -- all UI is transient and on-demand.

**Platforms:** Windows, macOS (Intel + Apple Silicon)

---

## Features

### Screenshot Capture

| Mode | Default Hotkey | Description |
|------|---------------|-------------|
| **Region** | `Alt+`` | Drag to select a rectangular area on a fullscreen overlay |
| **Fullscreen** | `Ctrl+Alt+Shift+D` | Captures screen(s) based on fullscreen mode setting |
| **Window** | `Ctrl+Alt+Shift+W` | Highlights and captures the window under your cursor |
| **OCR** | `Ctrl+Alt+Shift+T` | Select a region and extract text via native OS OCR engine |

> On macOS, `Ctrl` is replaced with `Cmd` in all hotkeys.

**Fullscreen capture modes:**
- **All screens** -- stitches all monitors into one image
- **Current screen** (default) -- captures the monitor the cursor is on
- **Select screen** -- overlay lets you click which monitor to capture

**Capture modifiers:**
- Hold **Shift** when releasing a region or clicking a window to force-open the annotation editor (even if disabled in settings)
- Hold **Alt** to toggle the save-to-disk behavior for that capture only (if saving is disabled, Alt forces a save; if enabled, Alt skips it)

**Capture delay:** Optional 3, 5, or 10-second countdown before capture (instant mode only), with on-screen timer and cancel via ESC.

### Overlay Modes

- **Instant** -- semi-transparent overlay drawn in real-time for fast interaction
- **Freeze** -- captures the screen first and displays the frozen image under the overlay, better for selecting dynamic content like video or animations
- **Window** -- freeze-style capture with real-time window boundary highlighting

### Screen Recording

| Format | Description |
|--------|-------------|
| **MP4** (H.264) | Hardware-accelerated via Media Foundation (Windows) or VideoToolbox (macOS), with openh264 software fallback |
| **GIF** | Configurable max duration and max width, with automatic downscaling and color quantization |

- **Record hotkey** (`Ctrl+Alt+Shift+R`) for quick start/stop toggle
- **Region recording** -- select a screen area to record
- Floating recording indicator with elapsed time and stop button
- Configurable frame rate (10/15/24/30 FPS)
- GPU encoder auto-selection: NVENC, AMF, QuickSync, or D3D11 on Windows; VideoToolbox on macOS; CPU fallback if needed

### Annotation Editor

Opens in its own resizable window (not a fullscreen overlay) so you can reference other content while annotating. Can be triggered after capture, or used to annotate existing files via the tray menu or Explorer context menu.

**Drawing tools (left-click and right-click each have independent mappings):**

| Tool | Description |
|------|-------------|
| **Freehand** | Brush strokes with configurable color and width |
| **Arrow** | Three styles: filled, hollow, and double-headed |
| **Oval / Ellipse** | Drag to define bounding box |
| **Rectangle** | Drag to define bounding box |
| **Text** | Click to place inline text input, draggable after placement, with black background for readability |
| **Blur** | Drag to select a region that gets a gaussian blur applied (for redacting sensitive content) |
| **Step numbering** | Sequential numbered circles for tutorial/workflow documentation |
| **Grab Text (OCR)** | Select a region and extract text via platform OCR |

**Modifier-key tool switching:** Each modifier (Shift, Ctrl, Alt) can be assigned a different tool independently for both left-click and right-click -- 8 configurable tool slots total.

**Editing features:**
- 100-step undo/redo (`Ctrl+Z` / `Ctrl+Shift+Z`)
- Color picker (hex input), stroke width (1-20px), font size (8-72pt), arrow style selector
- Crop tool with draggable edge/corner handles
- Delete annotations via red X button on hover
- Zoom (`Ctrl+scroll`) and pan (scroll, `Shift+scroll` horizontal, middle-mouse drag, or `Space+drag`)
- Reset view with `Ctrl+0`
- Draggable toolbar

Annotations are composited at **full original resolution**, not display resolution, so they stay sharp regardless of screen scaling.

### Output

- **Formats:** PNG (lossless, default), JPG (quality 85), WebP
- **Clipboard actions:** copy image, upload to catbox.moe and copy shareable URL, or none
- Screenshots are **always copied to clipboard**, with optional save to disk
- Configurable filename prefix and strftime date format (e.g., `screenshot_2026-02-28_14-30-00.png`)
- Automatic collision avoidance -- appends `_2`, `_3`, etc. if the file already exists
- Desktop notifications on save (clickable to reveal in file manager)

### Settings

Accessible from the system tray (left-click), organized into tabbed panels:

- **General** -- save folder (with real-time writability validation), save format, filename prefix/date format, clipboard action, save-to-disk toggle, capture mode (instant vs. freeze), fullscreen mode (all/current/select), capture delay, launch on startup, Explorer context menu toggle (Windows)
- **Shortcuts** -- hotkey bindings via inline key recorder (click to arm, press your combo) for region, fullscreen, window, OCR, and record
- **Recording** -- format (MP4/GIF), frame rate, GIF max duration and max width
- **Annotations** -- annotation toggle, left-click tool mappings (default + 3 modifiers), right-click tool mappings (default + 3 modifiers)
- **About** -- version display, in-app update checker

**Reset to defaults:** A footer button with a section picker dropdown lets you reset General, Shortcuts, Recording, Annotations, or All settings back to factory defaults with confirmation.

### System Tray

- Left-click opens Settings
- Right-click shows context menu:
  - All capture modes with hotkey labels
  - Record Screen / Record Region
  - OCR Region
  - Upload Last to catbox.moe
  - Annotate Image File (open file picker for existing images)
  - Last 5 captures -- click to reveal in file manager
  - Settings and Exit

### File Annotation

Annotate existing images on disk without capturing:
- **Tray menu:** "Annotate Image File..." opens a file picker (PNG, JPG, WebP, BMP)
- **Explorer context menu:** Right-click an image in Windows Explorer and select "Annotate with QuickShotter" (toggle in Settings > General)
- **CLI:** Launch with `--annotate <path>` to open directly into the annotation editor
- Annotated files save next to the original as `name-annotated.ext`

### Auto-Updater

- Check for updates directly from the About tab
- Downloads and installs updates with progress tracking
- Signed update bundles verified against a minisign public key
- Restart prompt after install -- no manual download needed

---

## Architecture

```
quickshotter/
├── index.html                 Settings window
├── overlay.html               Fullscreen capture overlay
├── annotation.html            Annotation editor
├── countdown.html             Capture delay countdown
├── recording-indicator.html   Recording timer/controls
├── src/
│   ├── main.ts                Settings UI, config sync, reset logic
│   ├── overlay.ts             Capture overlay, region selection, window highlighting
│   ├── annotation.ts          Annotation editor (8 tools, zoom/pan, crop, undo/redo)
│   ├── countdown.ts           Countdown timer for delayed captures
│   ├── recording-indicator.ts Recording timer, stop button, post-save UI
│   ├── types.ts               Shared TypeScript interfaces
│   └── styles.css             Settings styles
└── src-tauri/src/
    ├── main.rs                Entry point, CLI arg handling
    ├── lib.rs                 Tauri builder, plugin registration, window lifecycle
    ├── commands.rs            30+ IPC command handlers
    ├── capture.rs             Screenshot, multi-monitor stitching, clipboard, disk save
    ├── overlay.rs             Overlay and annotation window lifecycle
    ├── hotkeys.rs             Global shortcut registration with validation/rollback
    ├── tray.rs                System tray menu building and events
    ├── startup.rs             Launch-on-startup (Windows/macOS)
    ├── window_capture.rs      Background window detection worker thread
    ├── context_menu.rs        Windows Explorer context menu registration
    ├── ocr.rs                 Platform-native OCR (WinRT / Apple Vision)
    ├── imgur.rs               Upload to catbox.moe
    ├── config.rs              Config struct, JSON persistence, defaults
    ├── state.rs               AppState with mutex poison recovery
    ├── error.rs               Unified AppError enum
    └── recording/
        ├── pipeline.rs        Dual-thread capture/encode pipeline
        ├── encoder.rs         VideoEncoder trait, FallbackEncoder
        ├── encoder_win.rs     Media Foundation H.264 (Windows)
        ├── encoder_mac.rs     AVAssetWriter + VideoToolbox (macOS)
        ├── avwriter.m         Objective-C FFI for VideoToolbox
        ├── encoder_cpu.rs     openh264 software fallback
        ├── mp4_muxer.rs       Minimal ISOBMFF MP4 container writer
        └── gif_encoder.rs     GIF with downscaling and quantization
```

**Design principles:**

- Rust owns all screenshot data as `RgbaImage` objects in locked `AppState`
- The frontend only receives previews (base64 JPEG for overlay, base64 PNG for annotation)
- No persistent main window -- the app is tray-only between operations
- Single instance enforced via `tauri-plugin-single-instance`
- Atomic state transitions prevent duplicate captures from rapid hotkey presses

---

## Safeguards

### Capture Safety

- **Duplicate-capture prevention** -- atomic `is_capturing` flag; rapid hotkey presses are silently ignored
- **Multi-monitor DPI-aware stitching** -- detects per-monitor scale factors and normalizes to uniform physical resolution
- **Virtual desktop size limit** -- caps at 64 million pixels (~256MB RGBA) to prevent out-of-memory crashes
- **Minimum selection size** -- enforced in both Rust (3x3px) and TypeScript (3x3px)
- **Safe crop with bounds clamping** -- clamps coordinates before cropping to prevent panics from out-of-bounds values
- **State cleanup on every error path** -- `is_capturing` is always reset so captures are never permanently blocked
- **Single-monitor fast path** -- skips stitching logic for the common single-monitor case

### macOS-Specific

- **Blank screenshot detection** -- samples every ~997th pixel (prime stride to avoid repeating patterns); shows a permission notification if all pixels are black
- **Accessibility permission hint** -- hotkey registration failure includes a message directing the user to System Settings

### Input Validation and Security

- **Filename sanitization** -- strips path-traversal and reserved characters (`/\:<>|"?*\0` and `..`)
- **Path traversal safety net** -- extracts the final `file_name()` component after sanitization, guaranteeing no directory traversal
- **Folder writability test** -- creates and deletes a temporary file, not just an existence check
- **Date format validation** -- validated against chrono's strftime parser; falls back to a safe default on invalid input
- **Hotkey validation before persistence** -- parses and re-registers hotkeys before writing config; invalid hotkeys reject the entire save
- **Annotation base64 size limit** -- 134MB cap to prevent out-of-memory from oversized payloads
- **Content Security Policy** -- blocks external network requests; only allows `self` and `data:` URLs

### State Management

- **Mutex poison recovery** -- a custom `LockRecover` trait recovers from poisoned mutexes instead of panicking, preventing cascading thread crashes
- **Config atomicity** -- all validators run before any config is persisted; partial or broken state is never written to disk
- **Single instance enforcement** -- duplicate processes are silently terminated; CLI args forwarded to existing instance
- **Accidental exit prevention** -- window-close events are blocked; only the explicit Exit action from the tray menu can quit the app
- **Bounded capture history** -- capped at 5 entries to prevent unbounded growth

### Error Handling

- **Unified `AppError` enum** -- `Capture`, `Clipboard`, `Io`, `Config`, `Window`, `Annotation`, and `Tauri` variants with clear messages
- All IPC commands return `Result<T, AppError>`
- Inline error banners and real-time validation warnings in the settings UI
- Graceful degradation -- invalid states trigger fallbacks rather than crashes

---

## Performance

- **JPEG for previews** (quality 80, ~10x faster encoding), **PNG for annotation** (lossless)
- **Memory-efficient RGBA-to-RGB conversion** -- strips the alpha channel without cloning the source image (~33% less peak memory)
- **Persistent worker thread** for window detection -- avoids ~1ms thread-spawn overhead per cursor poll
- **Backpressure on window queries** -- an `AtomicBool` flag skips new polls if the previous query is still running
- **500ms timeout** on window detection queries to prevent indefinite hangs
- **Bounded recording pipeline** -- capture and encoder threads connected via bounded channel (4 frames max)
- **DPI-aware coordinate mapping** -- uses actual screenshot dimensions divided by canvas dimensions instead of relying on `devicePixelRatio` alone
- **Dev build optimization** -- third-party crates compiled at `opt-level = 2` even in debug builds, critical for the `image` crate's pixel processing

---

## Building from Source

### Prerequisites

- [Node.js](https://nodejs.org/) 20+
- [Rust](https://www.rust-lang.org/tools/install) (stable)

### Build and Run

```bash
npm install
npm run tauri dev    # Development
npm run tauri build  # Production
```

---

## CI/CD

GitHub Actions automatically builds on tag push (`v*`) for:

| Platform | Output |
|----------|--------|
| Windows x64 | NSIS installer (`.exe`) |
| macOS ARM | DMG (Apple Silicon) |
| macOS Intel | DMG (x86_64) |

Releases are auto-published with a download table. Each release includes signed update bundles and a `latest.json` manifest for the in-app auto-updater.

---

## Configuration

Config is stored in the platform-specific app data directory:

| Platform | Path |
|----------|------|
| Windows | `%APPDATA%\QuickShotter\config\config.json` |
| macOS | `~/Library/Application Support/com.quickshotter.app/config/config.json` |

All configuration changes are validated atomically before being persisted.

---

## Tech Stack

**Backend (Rust):**
tauri 2, image, arboard, xcap, chrono, base64, serde, thiserror, directories, openh264, gif, reqwest

**Frontend (TypeScript):**
@tauri-apps/api, @tauri-apps/plugin-global-shortcut, @tauri-apps/plugin-updater, @tauri-apps/plugin-process, vite, typescript

**Tauri Plugins:**
single-instance, global-shortcut, dialog, notification, autostart, updater, process
