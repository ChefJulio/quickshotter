use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder};

use crate::capture;
use crate::error::AppError;
use crate::state::{AppState, LockRecover};
use crate::window_capture;

// ── Daemon protocol types ──────────────────────────────────────────

#[derive(Serialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
enum DaemonCommand {
  Capture {
    mode: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    image: Option<String>,
    #[serde(skip_serializing_if = "is_zero_u32")]
    image_width: u32,
    #[serde(skip_serializing_if = "is_zero_u32")]
    image_height: u32,
    origin_x: i32,
    origin_y: i32,
    bounds_w: u32,
    bounds_h: u32,
  },
  #[allow(dead_code)]
  Cancel,
  Quit,
}

#[allow(dead_code)]
fn is_zero_u32(v: &u32) -> bool { *v == 0 }

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
#[serde(tag = "action", rename_all = "snake_case")]
enum DaemonResult {
  Ready,
  Region { x1: i32, y1: i32, x2: i32, y2: i32, shift: bool, alt: bool },
  Window { left: i32, top: i32, right: i32, bottom: i32, shift: bool, alt: bool },
  RecordRegion { x: i32, y: i32, width: u32, height: u32 },
  SelectScreen { monitor_index: u32 },
  Cancelled,
}

// ── Daemon state ───────────────────────────────────────────────────

pub struct OverlayDaemon {
  child: Child,
  stdin: ChildStdin,
  stdout: BufReader<ChildStdout>,
  #[allow(dead_code)]
  ready: bool,
}

impl OverlayDaemon {
  fn spawn() -> Result<Self, AppError> {
    let exe = find_overlay_binary()?;
    let mut child = Command::new(&exe)
      .stdin(Stdio::piped())
      .stdout(Stdio::piped())
      .stderr(Stdio::inherit())
      .spawn()
      .map_err(|e| AppError::Capture(format!("Failed to spawn overlay daemon at {}: {e}", exe.display())))?;

    let stdin = child.stdin.take()
      .ok_or_else(|| AppError::Capture("No stdin on overlay daemon".to_string()))?;
    let stdout = child.stdout.take()
      .ok_or_else(|| AppError::Capture("No stdout on overlay daemon".to_string()))?;
    let mut stdout = BufReader::new(stdout);

    // Wait for "ready" signal (GPU initialized)
    let mut line = String::new();
    stdout.read_line(&mut line)
      .map_err(|e| AppError::Capture(format!("Overlay daemon startup failed: {e}")))?;

    let ready = serde_json::from_str::<DaemonResult>(&line)
      .map(|r| matches!(r, DaemonResult::Ready))
      .unwrap_or(false);

    if !ready {
      return Err(AppError::Capture(format!("Overlay daemon did not report ready: {}", line.trim())));
    }

    Ok(Self { child, stdin, stdout, ready: true })
  }

  fn send(&mut self, cmd: &DaemonCommand) -> Result<(), AppError> {
    let json = serde_json::to_string(cmd)
      .map_err(|e| AppError::Capture(format!("Failed to serialize command: {e}")))?;
    writeln!(self.stdin, "{json}")
      .map_err(|e| AppError::Capture(format!("Failed to write to daemon stdin: {e}")))?;
    self.stdin.flush()
      .map_err(|e| AppError::Capture(format!("Failed to flush daemon stdin: {e}")))?;
    Ok(())
  }

  fn read_result(&mut self) -> Result<DaemonResult, AppError> {
    let mut line = String::new();
    self.stdout.read_line(&mut line)
      .map_err(|e| AppError::Capture(format!("Failed to read daemon stdout: {e}")))?;
    serde_json::from_str(&line)
      .map_err(|e| AppError::Capture(format!("Invalid daemon response '{}': {e}", line.trim())))
  }

  fn is_alive(&mut self) -> bool {
    matches!(self.child.try_wait(), Ok(None))
  }

  fn kill(&mut self) {
    let _ = self.send(&DaemonCommand::Quit);
    let _ = self.child.kill();
    let _ = self.child.wait();
  }
}

impl Drop for OverlayDaemon {
  fn drop(&mut self) {
    self.kill();
  }
}

// ── Daemon lifecycle ───────────────────────────────────────────────

/// Global daemon instance. Protected by its own mutex (separate from AppState
/// to avoid holding AppState lock during blocking I/O with the daemon).
static DAEMON: Mutex<Option<OverlayDaemon>> = Mutex::new(None);

/// Spawn the overlay daemon. Called once at app startup to pre-warm the GPU.
pub fn spawn_daemon() {
  std::thread::spawn(|| {
    match OverlayDaemon::spawn() {
      Ok(daemon) => {
        eprintln!("Overlay daemon started (GPU pre-warmed)");
        *DAEMON.lock().unwrap_or_else(|e| e.into_inner()) = Some(daemon);
      }
      Err(e) => {
        eprintln!("Failed to start overlay daemon: {e}");
        // Non-fatal: overlay will try to spawn on first capture
      }
    }
  });
}

/// Get or spawn the daemon. Returns the lock guard.
fn get_daemon() -> Result<std::sync::MutexGuard<'static, Option<OverlayDaemon>>, AppError> {
  let mut guard = DAEMON.lock().unwrap_or_else(|e| e.into_inner());

  // Check if daemon is alive, respawn if not
  let needs_spawn = match guard.as_mut() {
    Some(d) => !d.is_alive(),
    None => true,
  };

  if needs_spawn {
    eprintln!("Overlay daemon not running, spawning...");
    *guard = Some(OverlayDaemon::spawn()?);
  }

  Ok(guard)
}

/// Shut down the daemon. Called on app exit.
#[allow(dead_code)]
pub fn shutdown_daemon() {
  if let Ok(mut guard) = DAEMON.lock() {
    if let Some(daemon) = guard.as_mut() {
      daemon.kill();
    }
    *guard = None;
  }
}

// ── Public overlay API (same signatures as before) ─────────────────

pub fn open_overlay(app: &AppHandle) -> Result<(), AppError> {
  let mode = app.state::<Mutex<AppState>>().lock_or_recover().config.capture_mode.clone();
  open_overlay_with_mode(app, &mode)
}

pub fn open_overlay_with_mode(app: &AppHandle, mode: &str) -> Result<(), AppError> {
  let t0 = std::time::Instant::now();
  eprintln!("[overlay] open_overlay_with_mode called, mode={mode}");

  // Atomically check and set is_capturing
  {
    let s = app.state::<Mutex<AppState>>();
    let mut state = s.lock_or_recover();
    if state.is_capturing || state.is_annotating || state.is_recording {
      eprintln!("[overlay] blocked: is_capturing={} is_annotating={} is_recording={}",
        state.is_capturing, state.is_annotating, state.is_recording);
      return Ok(());
    }
    state.is_capturing = true;
    state.overlay_mode = mode.to_string();
  }

  // macOS: check screen recording permission before proceeding.
  // For production builds, the preflight check is reliable.
  // For dev builds, it may fail — the blank-capture fallback below handles that.
  #[cfg(target_os = "macos")]
  if !capture::ensure_screen_recording_permission(app) {
    app.state::<Mutex<AppState>>().lock_or_recover().is_capturing = false;
    return Ok(());
  }

  // Get virtual desktop bounds — use cached if available (avoids Monitor::all() OS call)
  let bounds = {
    let s = app.state::<Mutex<AppState>>();
    let state = s.lock_or_recover();
    state.cached_bounds.map(|(x, y, w, h)| capture::DesktopBounds { x, y, width: w, height: h })
  };
  let bounds = match bounds {
    Some(b) => b,
    None => match capture::get_desktop_bounds() {
      Ok(b) => b,
      Err(e) => {
        app.state::<Mutex<AppState>>().lock_or_recover().is_capturing = false;
        return Err(e);
      }
    }
  };
  eprintln!("[overlay] desktop bounds: origin=({},{}) size={}x{}", bounds.x, bounds.y, bounds.width, bounds.height);

  // Cache bounds so completion handlers don't re-enumerate monitors
  app.state::<Mutex<AppState>>().lock_or_recover().cached_bounds =
    Some((bounds.x, bounds.y, bounds.width, bounds.height));

  // Start window detection worker for window mode
  if mode == "window" {
    window_capture::start();
  }

  // Pre-capture for freeze mode (and all modes on macOS)
  let is_recording = mode == "record_region";
  let needs_capture = !is_recording && (mode == "freeze" || cfg!(target_os = "macos"));

  let mut image_path: Option<String> = None;
  let mut image_width: u32 = 0;
  let mut image_height: u32 = 0;

  if needs_capture {
    let screen = match capture::capture_all_monitors() {
      Ok(s) => s,
      Err(e) => {
        // On macOS, capture failure likely means permission was revoked
        #[cfg(target_os = "macos")]
        {
          capture::notify_blank_capture(app);
          app.state::<Mutex<AppState>>().lock_or_recover().is_capturing = false;
          return Ok(());
        }
        #[cfg(not(target_os = "macos"))]
        {
          app.state::<Mutex<AppState>>().lock_or_recover().is_capturing = false;
          return Err(e);
        }
      }
    };

    #[cfg(target_os = "macos")]
    if capture::is_likely_blank(&screen.image) {
      capture::notify_blank_capture(app);
      app.state::<Mutex<AppState>>().lock_or_recover().is_capturing = false;
      return Ok(());
    }

    // Write raw RGBA to temp file for the overlay daemon
    if mode == "freeze" {
      let temp = std::env::temp_dir().join("qs_overlay_capture.raw");
      if let Err(e) = std::fs::write(&temp, screen.image.as_raw()) {
        app.state::<Mutex<AppState>>().lock_or_recover().is_capturing = false;
        return Err(AppError::Capture(format!("Failed to write temp capture: {e}")));
      }
      image_path = Some(temp.to_string_lossy().to_string());
      image_width = screen.image.width();
      image_height = screen.image.height();
    }

    // Store pending screenshot for cropping after selection
    let s = app.state::<Mutex<AppState>>();
    let mut state = s.lock_or_recover();
    state.pending_screenshot = Some(screen.image);
    // No base64 needed anymore — the daemon renders the texture directly
  
  }

  eprintln!("[overlay] ready to send in {:?}, mode={mode}", t0.elapsed());

  // macOS: use webview overlay (Tauri window with overlay.html).
  // The native daemon overlay has platform-specific rendering that doesn't
  // work on macOS. The webview path handles all macOS quirks natively.
  #[cfg(target_os = "macos")]
  {
    // For freeze mode on macOS, encode screenshot as base64 JPEG for the webview
    if mode == "freeze" || mode == "live" || mode == "instant" {
      let b64 = {
        let s = app.state::<Mutex<AppState>>();
        let state = s.lock_or_recover();
        state.pending_screenshot.as_ref().map(|img| {
          let rgb_data: Vec<u8> = img.as_raw().chunks_exact(4).flat_map(|px| [px[0], px[1], px[2]]).collect();
          let mut jpeg_buf = Vec::new();
          let encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg_buf, 80);
          match image::ImageEncoder::write_image(encoder, &rgb_data, img.width(), img.height(), image::ExtendedColorType::Rgb8) {
            Ok(()) => {
              use base64::Engine;
              Some(base64::engine::general_purpose::STANDARD.encode(&jpeg_buf))
            }
            Err(e) => {
              eprintln!("[overlay] JPEG encode failed: {e}");
              None
            }
          }
        }).flatten()
      }; // lock dropped here
      if let Some(b64) = b64 {
        app.state::<Mutex<AppState>>().lock_or_recover().pending_screenshot_base64 = Some(b64);
      }
    }

    eprintln!("[overlay] calling open_webview_overlay");
    let result = open_webview_overlay(app, mode, &bounds);
    eprintln!("[overlay] open_webview_overlay result: {:?}", result.as_ref().map(|_| "ok").unwrap_or("err"));
    if let Err(ref e) = result {
      eprintln!("[overlay] webview overlay error: {e}");
    }
    return result;
  }

  // Windows: use native daemon overlay
  #[cfg(not(target_os = "macos"))]
  {
    let daemon_mode = mode.to_string();
    let app2 = app.clone();

    std::thread::spawn(move || {
      eprintln!("[overlay] background thread: calling run_capture_on_daemon");
      let result = run_capture_on_daemon(
        &daemon_mode, image_path, image_width, image_height,
        bounds.x, bounds.y, bounds.width, bounds.height,
      );

      match result {
        Ok(ref daemon_result) => {
          eprintln!("[overlay] daemon result received: {daemon_result:?}");
          dispatch_result(&app2, result.unwrap());
        }
        Err(e) => {
          eprintln!("[overlay] daemon error: {e}");
          reset_capture_state(&app2);
        }
      }
    });

    Ok(())
  }
}

/// macOS: open the webview-based overlay window.
#[cfg(target_os = "macos")]
fn open_webview_overlay(app: &AppHandle, mode: &str, bounds: &capture::DesktopBounds) -> Result<(), AppError> {
  // On macOS, xcap returns logical points for monitor dimensions,
  // and Tauri's position/inner_size also expect logical points.
  // No scale factor conversion needed.
  let label = "overlay";

  // Destroy any leftover overlay window from a previous capture
  if let Some(existing) = app.get_webview_window(label) {
    existing.destroy().ok();
  }

  match WebviewWindowBuilder::new(app, label, WebviewUrl::App("overlay.html".into()))
    .title("QuickShotter Overlay")
    .decorations(false)
    .transparent(true)
    .always_on_top(true)
    .resizable(false)
    .skip_taskbar(true)
    .position(bounds.x as f64, bounds.y as f64)
    .inner_size(bounds.width as f64, bounds.height as f64)
    .build()
  {
    Ok(_win) => {
      eprintln!("[overlay] webview overlay created ({}x{})", bounds.width, bounds.height);
      Ok(())
    }
    Err(e) => {
      // Reset state so the app doesn't get stuck
      app.state::<Mutex<AppState>>().lock_or_recover().is_capturing = false;
      Err(AppError::Capture(format!("Failed to create overlay window: {e}")))
    }
  }
}

/// Send a capture command to the daemon and wait for the result.
/// This blocks the calling thread (which is why it runs on a background thread).
fn run_capture_on_daemon(
  mode: &str,
  image: Option<String>,
  image_width: u32,
  image_height: u32,
  origin_x: i32,
  origin_y: i32,
  bounds_w: u32,
  bounds_h: u32,
) -> Result<DaemonResult, AppError> {
  let mut guard = get_daemon()?;
  let daemon = guard.as_mut()
    .ok_or_else(|| AppError::Capture("Daemon not available".to_string()))?;

  let cmd = DaemonCommand::Capture {
    mode: mode.to_string(),
    image,
    image_width,
    image_height,
    origin_x,
    origin_y,
    bounds_w,
    bounds_h,
  };

  daemon.send(&cmd)?;
  daemon.read_result()
}

/// Dispatch the daemon's result to the existing capture pipeline.
///
/// IMPORTANT: By the time we get here, the daemon has already destroyed its
/// overlay window. For live/instant mode (no pending_screenshot), we need a
/// short delay to let the compositor finish removing the window before we
/// capture the screen fresh.
fn dispatch_result(app: &AppHandle, result: DaemonResult) {
  match result {
    DaemonResult::Ready => { /* shouldn't happen here */ }
    DaemonResult::Region { x1, y1, x2, y2, shift, alt } => {
      // Check if the overlay was opened in OCR mode — route accordingly.
      let overlay_mode = app.state::<Mutex<AppState>>().lock_or_recover().overlay_mode.clone();
      let app = app.clone();
      tauri::async_runtime::spawn(async move {
        let result = if overlay_mode == "ocr" {
          // Convert screen coords to image coords (same as region capture)
          let (bx, by) = {
            let s = app.state::<Mutex<AppState>>();
            let state = s.lock_or_recover();
            match state.cached_bounds {
              Some((x, y, _, _)) => (x, y),
              None => {
                drop(state);
                let b = crate::capture::get_desktop_bounds().unwrap_or(
                  crate::capture::DesktopBounds { x: 0, y: 0, width: 0, height: 0 }
                );
                (b.x, b.y)
              }
            }
          };
          let img_x1 = ((x1.min(x2) - bx).max(0)) as u32;
          let img_y1 = ((y1.min(y2) - by).max(0)) as u32;
          let img_x2 = ((x1.max(x2) - bx).max(0)) as u32;
          let img_y2 = ((y1.max(y2) - by).max(0)) as u32;
          crate::commands::complete_ocr_capture_from_overlay(&app, img_x1, img_y1, img_x2, img_y2)
            .await.map(|_| ())
        } else {
          crate::commands::complete_region_capture_from_overlay(
            &app, x1, y1, x2, y2, shift, alt,
          ).await.map(|_| ())
        };
        if let Err(e) = result {
          eprintln!("Region capture failed: {e}");
        }
        close_overlay(&app);
      });
    }
    DaemonResult::Window { left, top, right, bottom, shift, alt } => {
      let app = app.clone();
      tauri::async_runtime::spawn(async move {
        let result = crate::commands::complete_window_capture_from_overlay(
          &app, left, top, right, bottom, shift, alt,
        ).await;
        if let Err(e) = result {
          eprintln!("Window capture failed: {e}");
        }
        close_overlay(&app);
      });
    }
    DaemonResult::RecordRegion { x, y, width, height } => {
      let app = app.clone();
      tauri::async_runtime::spawn(async move {
        eprintln!("Record region: {}x{} at ({}, {})", width, height, x, y);
        close_overlay(&app);
      });
    }
    DaemonResult::SelectScreen { monitor_index } => {
      let app = app.clone();
      tauri::async_runtime::spawn(async move {
        let result = crate::commands::complete_select_screen_capture(
          app.clone(), monitor_index as usize,
        ).await;
        if let Err(e) = result {
          eprintln!("Select screen capture failed: {e}");
        }
        close_overlay(&app);
      });
    }
    DaemonResult::Cancelled => {
      close_overlay(app);
    }
  }
}

fn reset_capture_state(app: &AppHandle) {
  let s = app.state::<Mutex<AppState>>();
  let mut state = s.lock_or_recover();
  state.is_capturing = false;
  state.pending_screenshot = None;

  state.cached_bounds = None;
}

pub fn close_overlay(app: &AppHandle) {
  window_capture::stop();
  // Close the webview overlay window (macOS path)
  if let Some(win) = app.get_webview_window("overlay") {
    win.destroy().ok();
  }
  reset_capture_state(app);
  // Clean up temp file
  let temp = std::env::temp_dir().join("qs_overlay_capture.raw");
  let _ = std::fs::remove_file(temp);
}

/// Find the overlay binary relative to the main executable.
fn find_overlay_binary() -> Result<PathBuf, AppError> {
  let exe = std::env::current_exe()
    .map_err(|e| AppError::Capture(format!("Cannot find exe path: {e}")))?;
  let exe_dir = exe.parent()
    .ok_or_else(|| AppError::Capture("Exe has no parent dir".to_string()))?;

  // Production: next to main exe
  let prod = exe_dir.join("quickshotter-overlay.exe");
  if prod.exists() { return Ok(prod); }

  // Also check without .exe (macOS/Linux)
  let prod_unix = exe_dir.join("quickshotter-overlay");
  if prod_unix.exists() { return Ok(prod_unix); }

  // Development: walk up from src-tauri/target/debug/ to find overlay/target/debug/
  let mut dir = exe_dir;
  for _ in 0..5 {
    // Check for overlay/target/debug/ or overlay/target/release/
    let dev_debug = dir.join("overlay").join("target").join("debug").join("quickshotter-overlay.exe");
    if dev_debug.exists() { return Ok(dev_debug); }
    let dev_release = dir.join("overlay").join("target").join("release").join("quickshotter-overlay.exe");
    if dev_release.exists() { return Ok(dev_release); }
    // Unix variants
    let dev_debug_unix = dir.join("overlay").join("target").join("debug").join("quickshotter-overlay");
    if dev_debug_unix.exists() { return Ok(dev_debug_unix); }
    let dev_release_unix = dir.join("overlay").join("target").join("release").join("quickshotter-overlay");
    if dev_release_unix.exists() { return Ok(dev_release_unix); }

    match dir.parent() {
      Some(p) => dir = p,
      None => break,
    }
  }

  Err(AppError::Capture(
    "Overlay binary not found. Build it with: cd overlay && cargo build".to_string()
  ))
}

// ── Annotation window (unchanged) ──────────────────────────────────

pub fn open_annotation_window(app: &AppHandle) -> Result<(), AppError> {
  let (x, y, w, h) = if let Ok(Some(monitor)) = app.primary_monitor() {
    let pos = monitor.position();
    let size = monitor.size();
    let sf = monitor.scale_factor();
    let mon_w = size.width as f64 / sf;
    let mon_h = size.height as f64 / sf;
    let win_w = (mon_w * 0.8).round();
    let win_h = (mon_h * 0.8).round();
    let win_x = pos.x as f64 + (mon_w - win_w) / 2.0;
    let win_y = pos.y as f64 + (mon_h - win_h) / 2.0;
    (win_x, win_y, win_w, win_h)
  } else {
    let bounds = capture::get_desktop_bounds()?;
    let w = (bounds.width as f64 * 0.8).round();
    let h = (bounds.height as f64 * 0.8).round();
    let x = bounds.x as f64 + (bounds.width as f64 - w) / 2.0;
    let y = bounds.y as f64 + (bounds.height as f64 - h) / 2.0;
    (x, y, w, h)
  };
  WebviewWindowBuilder::new(app, "annotation", WebviewUrl::App("annotation.html".into()))
    .title("QuickShotter - Annotate")
    .decorations(true)
    .resizable(true)
    .position(x, y)
    .inner_size(w, h)
    .min_inner_size(400.0, 300.0)
    .visible(false)
    .build()?;
  Ok(())
}

pub fn close_annotation_window(app: &AppHandle) {
  if let Some(win) = app.get_webview_window("annotation") {
    if let Err(e) = win.destroy() {
      eprintln!("Failed to destroy annotation window: {e}");
    }
  }
  let s = app.state::<Mutex<AppState>>();
  let mut state = s.lock_or_recover();
  // Clean up temp annotation file
  if let Some(ref path) = state.pending_annotation_path {
    let _ = std::fs::remove_file(path);
  }
  state.pending_annotation = None;
  state.pending_annotation_path = None;
  state.annotation_source_path = None;
  state.is_annotating = false;
}
