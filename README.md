# QuickShotter

A lightweight, cross-platform screenshot and screen recording tool built with Tauri 2, Rust, and TypeScript. Lives in your system tray with no persistent window — all UI is transient and on-demand.

**Platforms:** Windows, macOS (Intel + Apple Silicon), Linux

---

## Features

### Screenshot Capture

| Mode | Default Hotkey | Description |
|------|---------------|-------------|
| **Region** | `Ctrl+Alt+Shift+S` | Drag to select a rectangular area on a fullscreen overlay |
| **Fullscreen** | `Ctrl+Alt+Shift+D` | Instantly captures all monitors, stitched into one image |
| **Window** | `Ctrl+Alt+Shift+W` | Highlights and captures the window under your cursor |

> On macOS, `Ctrl` is replaced with `Cmd` in all hotkeys.

### Screen Recording

| Format | Description |
|--------|-------------|
| **MP4** (H.264) | Hardware-accelerated via Media Foundation (Windows), VideoToolbox (macOS), or software fallback via openh264 |
| **GIF** | Configurable max duration and max width, with automatic downscaling and color quantization |

- **Record hotkey** for quick start/stop toggle
- **Region recording** — select a screen area to record
- Configurable frame rate (10/15/24/30 FPS)
- GPU encoder auto-selection: NVENC, AMF, QuickSync, or D3D11 on Windows; VideoToolbox on macOS; CPU fallback on all platforms

### Overlay Modes

- **Instant** — semi-transparent overlay drawn in real-time for fast interaction
- **Freeze** — captures the screen first and displays the frozen image under the overlay, better for selecting dynamic content like video or animations
- **Window** — freeze-style capture with real-time window boundary highlighting

### Annotation Editor

An optional built-in editor opens after capture with five drawing tools:

- **Freehand** brush strokes
- **Arrow** with three styles: filled, hollow, and double-headed
- **Oval / Ellipse**
- **Rectangle**
- **Text** with inline input, draggable after placement

**Editing features:**
- 100-step undo/redo (`Ctrl+Z` / `Ctrl+Shift+Z`)
- Color picker, stroke width (1-20px), font size (8-72pt)
- Arrow style selector
- Draggable toolbar
- Configurable modifier-key tool switching — assign any tool (or none) to Shift, Ctrl, and Alt independently

Annotations are composited at **full original resolution**, not display resolution, so they stay sharp regardless of screen scaling.

### Output

- **Formats:** PNG (lossless, default), JPG (quality 85), WebP
- Screenshots are **always copied to clipboard**, with optional save to disk
- Configurable filename prefix and strftime date format (e.g., `screenshot_2026-02-28_14-30-00.png`)
- Automatic collision avoidance — appends `_2`, `_3`, etc. if the file already exists

### Settings

Accessible from the system tray, organized into tabbed panels:

- **General** — save folder (with real-time writability validation), save format, filename prefix/date format, clipboard and save-to-disk toggles, capture mode (instant vs. freeze), launch on startup
- **Shortcuts** — hotkey bindings via inline key recorder (click to arm, press your combo) for region, fullscreen, window, and record
- **Recording** — format (MP4/GIF), frame rate, GIF max duration and max width
- **Annotations** — annotation toggle and tool mappings for each modifier key
- **About** — version display, in-app update checker

### Auto-Updater

- Check for updates directly from the About tab
- Downloads and installs updates with progress tracking
- Signed update bundles verified against a minisign public key
- Restart prompt after install — no manual download needed

### System Tray

- Quick access to all three capture modes with hotkey labels
- Last 5 captures — click to reveal in your file manager
- Settings and Exit

---

## Architecture

```
quickshotter/
├── index.html                 Settings window
├── overlay.html               Fullscreen capture overlay
├── annotation.html            Annotation editor
├── src/
│   ├── main.ts                Settings UI
│   ├── overlay.ts             Capture overlay logic
│   ├── annotation.ts          Annotation editor (5 tools, undo/redo)
│   ├── types.ts               Shared TypeScript interfaces
│   └── styles.css             Settings styles
└── src-tauri/src/
    ├── main.rs                Entry point
    ├── lib.rs                 Tauri builder and plugin registration
    ├── commands.rs            IPC commands
    ├── capture.rs             Screenshot, clipboard, disk save
    ├── overlay.rs             Overlay window lifecycle
    ├── hotkeys.rs             Global shortcut registration
    ├── tray.rs                System tray menu
    ├── startup.rs             Launch-on-startup (cross-platform)
    ├── window_capture.rs      Window detection worker thread
    ├── config.rs              Config struct and JSON persistence
    ├── state.rs               AppState with mutex poison recovery
    ├── error.rs               Unified AppError enum
    └── recording/
        ├── mod.rs             Module exports and type re-exports
        ├── pipeline.rs        Recording region, format, capture source
        ├── encoder.rs         VideoEncoder trait, FallbackEncoder
        ├── encoder_win.rs     Media Foundation H.264 (Windows)
        ├── encoder_mac.rs     AVAssetWriter + VideoToolbox (macOS)
        ├── avwriter.m         Objective-C FFI for VideoToolbox
        ├── encoder_cpu.rs     openh264 software fallback
        ├── mp4_muxer.rs       Minimal ISOBMFF MP4 container
        └── gif_encoder.rs     GIF with downscaling and quantization
```

**Design principles:**

- Rust owns all screenshot data as `RgbaImage` objects in locked `AppState`
- The frontend only receives previews (base64 JPEG for overlay, base64 PNG for annotation)
- No persistent main window — the app is tray-only between operations
- Single instance enforced via `tauri-plugin-single-instance`
- Atomic state transitions prevent duplicate captures from rapid hotkey presses

---

## Safeguards

### Capture Safety

- **Duplicate-capture prevention** — atomic `is_capturing` flag; rapid hotkey presses are silently ignored
- **Multi-monitor DPI-aware stitching** — detects per-monitor scale factors and normalizes to uniform physical resolution
- **Virtual desktop size limit** — caps at 64 million pixels (~256MB RGBA) to prevent out-of-memory crashes
- **Minimum selection size** — enforced in both Rust (3x3px) and TypeScript (3x3px)
- **Safe crop with bounds clamping** — clamps coordinates before cropping to prevent panics from out-of-bounds values
- **State cleanup on every error path** — `is_capturing` is always reset so captures are never permanently blocked
- **Single-monitor fast path** — skips stitching logic for the common single-monitor case

### macOS-Specific

- **Blank screenshot detection** — samples every ~997th pixel (prime stride to avoid repeating patterns); shows a permission notification if all pixels are black
- **Accessibility permission hint** — hotkey registration failure includes a message directing the user to System Settings

### Input Validation and Security

- **Filename sanitization** — strips path-traversal and reserved characters (`/\:<>|"?*\0` and `..`)
- **Path traversal safety net** — extracts the final `file_name()` component after sanitization, guaranteeing no directory traversal
- **Folder writability test** — creates and deletes a temporary file, not just an existence check
- **Date format validation** — validated against chrono's strftime parser; falls back to a safe default on invalid input
- **Hotkey validation before persistence** — parses and re-registers hotkeys before writing config; invalid hotkeys reject the entire save
- **Annotation base64 size limit** — 134MB cap to prevent out-of-memory from oversized payloads
- **Content Security Policy** — blocks external network requests; only allows `self` and `data:` URLs

### State Management

- **Mutex poison recovery** — a custom `LockRecover` trait recovers from poisoned mutexes instead of panicking, preventing cascading thread crashes
- **Config atomicity** — all validators run before any config is persisted; partial or broken state is never written to disk
- **Single instance enforcement** — duplicate processes are silently terminated
- **Accidental exit prevention** — window-close events are blocked; only the explicit Exit action from the tray menu can quit the app
- **Bounded capture history** — capped at 5 entries to prevent unbounded growth

### Error Handling

- **Unified `AppError` enum** — `Capture`, `Clipboard`, `Io`, `Config`, `Window`, `Annotation`, and `Tauri` variants with clear messages
- All IPC commands return `Result<T, AppError>`
- Inline error banners and real-time validation warnings in the settings UI
- Graceful degradation — invalid states trigger fallbacks rather than crashes

---

## Performance

- **JPEG for previews** (quality 80, ~10x faster encoding), **PNG for annotation** (lossless)
- **Memory-efficient RGBA-to-RGB conversion** — strips the alpha channel without cloning the source image (~33% less peak memory)
- **Persistent worker thread** for window detection — avoids ~1ms thread-spawn overhead per cursor poll
- **Backpressure on window queries** — an `AtomicBool` flag skips new polls if the previous query is still running
- **500ms timeout** on window detection queries to prevent indefinite hangs
- **DPI-aware coordinate mapping** — uses actual screenshot dimensions divided by canvas dimensions instead of relying on `devicePixelRatio` alone
- **Dev build optimization** — third-party crates compiled at `opt-level = 2` even in debug builds, critical for the `image` crate's pixel processing

---

## Building from Source

### Prerequisites

- [Node.js](https://nodejs.org/) 20+
- [Rust](https://www.rust-lang.org/tools/install) (stable)
- Platform-specific dependencies (see below)

### Linux Dependencies

```bash
sudo apt install libwebkit2gtk-4.1-dev libappindicator3-dev librsvg2-dev \
  patchelf libxdo-dev libxcb1-dev libxcb-render0-dev libxcb-shape0-dev \
  libxcb-xfixes0-dev libxcb-shm0-dev libxcb-randr0-dev \
  libxcb-composite0-dev libpipewire-0.3-dev libgbm-dev libdrm-dev \
  libegl1-mesa-dev
```

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
| Linux | DEB + AppImage |

Releases are auto-published with a download table. Each release includes signed update bundles and a `latest.json` manifest for the in-app auto-updater.

---

## Configuration

Config is stored in the platform-specific app data directory:

| Platform | Path |
|----------|------|
| Windows | `%APPDATA%\QuickShotter\config\config.json` |
| macOS | `~/Library/Application Support/com.quickshotter.app/config/config.json` |
| Linux | `~/.config/quickshotter/config/config.json` |

All configuration changes are validated atomically before being persisted.

---

## Tech Stack

**Backend (Rust):**
tauri 2, image, arboard, xcap, chrono, base64, serde, thiserror, directories, openh264, gif

**Frontend (TypeScript):**
@tauri-apps/api, @tauri-apps/plugin-global-shortcut, @tauri-apps/plugin-updater, @tauri-apps/plugin-process, vite, typescript

**Tauri Plugins:**
single-instance, global-shortcut, dialog, notification, autostart, updater, process
