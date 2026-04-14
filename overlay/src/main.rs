//! QuickShotter native overlay daemon.
//!
//! Per-monitor overlay windows: each monitor gets its own window + renderer
//! to stay within GPU surface size limits (8192px) on multi-monitor Retina setups.
//!
//! Two rendering paths:
//! - Freeze mode: opaque wgpu window, screenshot texture on GPU
//! - Live/Window/SelectScreen/etc: transparent wgpu surface (macOS Metal)
//!   or Win32 layered window (Windows)

mod gpu;
mod highlight;
mod interaction;
mod live_renderer;
mod protocol;
mod renderer;
mod window;
mod window_detect;

use std::io::BufRead;
use std::sync::mpsc;
use std::sync::Arc;

use winit::application::ApplicationHandler;
use winit::event::{ElementState, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::window::WindowId;

use gpu::GpuContext;
#[cfg(target_os = "windows")]
use highlight::HighlightBorder;
use interaction::{Interaction, MonitorRect, Phase, Rect};
use live_renderer::LiveRenderer;
use protocol::{CaptureMode, CaptureRequest, Command, OverlayResult};
use renderer::{FreezeRenderer, LiveGpuRenderer};
use window_detect::WindowDetector;

enum StdinMsg {
  Capture(CaptureRequest),
  ShowBorder(protocol::BorderRequest),
  HideBorder,
  Cancel,
  Quit,
}

fn main() {
  // macOS: ensure this process can become the active application and receive
  // events. Without this, a child process's windows won't get mouse/keyboard
  // events because macOS doesn't deliver events to background processes.
  #[cfg(target_os = "macos")]
  {
    use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy};
    unsafe {
      let mtm = objc2::MainThreadMarker::new_unchecked();
      let app = NSApplication::sharedApplication(mtm);
      // Regular policy required for winit event loop to work, then switch
      // to Accessory to hide from Cmd+Tab. Activation happens per-capture
      // in apply_macos_fixups when the window is shown.
      app.setActivationPolicy(NSApplicationActivationPolicy::Regular);
      app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
    }
    eprintln!("[daemon] NSApp initialized (Accessory)");
  }

  let gpu = match GpuContext::init() {
    Ok(g) => Arc::new(g),
    Err(e) => {
      eprintln!("GPU init failed: {e}");
      std::process::exit(1);
    }
  };

  OverlayResult::Ready.send();

  let event_loop = EventLoop::<()>::with_user_event()
    .build()
    .expect("failed to create event loop");
  event_loop.set_control_flow(ControlFlow::Wait);
  let proxy = event_loop.create_proxy();

  let (tx, rx) = mpsc::channel::<StdinMsg>();
  std::thread::spawn(move || stdin_reader(tx, proxy));

  let mut app = OverlayApp {
    gpu,
    rx,
    state: AppState::Idle,
  };

  event_loop.run_app(&mut app).ok();
}

fn stdin_reader(tx: mpsc::Sender<StdinMsg>, proxy: winit::event_loop::EventLoopProxy<()>) {
  let stdin = std::io::stdin().lock();
  for line in stdin.lines() {
    let line = match line {
      Ok(l) => l,
      Err(_) => break,
    };
    if line.trim().is_empty() {
      continue;
    }
    match serde_json::from_str::<Command>(&line) {
      Ok(Command::Capture(req)) => {
        eprintln!("[overlay-daemon] capture command: mode={:?}", req.mode);
        if tx.send(StdinMsg::Capture(req)).is_err() { break; }
      }
      Ok(Command::ShowBorder(req)) => {
        eprintln!("[overlay-daemon] show_border: {}x{} at ({},{})", req.width, req.height, req.x, req.y);
        if tx.send(StdinMsg::ShowBorder(req)).is_err() { break; }
      }
      Ok(Command::HideBorder) => {
        eprintln!("[overlay-daemon] hide_border");
        let _ = tx.send(StdinMsg::HideBorder);
      }
      Ok(Command::Cancel) => { let _ = tx.send(StdinMsg::Cancel); }
      Ok(Command::Quit) => {
        let _ = tx.send(StdinMsg::Quit);
        let _ = proxy.send_event(());
        break;
      }
      Err(e) => {
        eprintln!("[overlay-daemon] invalid command: {e}");
        // Send cancelled so the host doesn't hang
        OverlayResult::Cancelled.send();
        continue;
      }
    }
    let _ = proxy.send_event(());
  }
  let _ = tx.send(StdinMsg::Quit);
  let _ = proxy.send_event(());
}

// ── Renderer enum ──────────────────────────────────────────────────

enum ActiveRenderer {
  Freeze(FreezeRenderer),
  #[cfg(target_os = "windows")]
  Live(LiveRenderer),
  LiveGpu(LiveGpuRenderer),
  /// Window mode: no per-pixel rendering, just a nearly-invisible overlay
  /// that captures mouse events. The highlight is drawn via separate border windows.
  WindowMode,
}

// ── Per-monitor overlay ────────────────────────────────────────────

struct MonitorOverlay {
  window: Arc<winit::window::Window>,
  renderer: ActiveRenderer,
  /// This monitor's origin in logical screen points (from xcap).
  logical_origin_x: f32,
  logical_origin_y: f32,
  /// Logical dimensions (from xcap).
  logical_width: f32,
  logical_height: f32,
  /// Window scale factor (e.g. 2.0 on Retina, 1.0 on non-Retina).
  window_scale: f32,
  /// Freeze mode: native-resolution capture for final crop.
  freeze_capture: Option<image::RgbaImage>,
}

// ── App state machine ──────────────────────────────────────────────

enum AppState {
  Idle,
  /// Recording border — click-through window showing a red border.
  Border {
    window: Arc<winit::window::Window>,
    renderer: LiveGpuRenderer,
  },
  Active {
    monitors: Vec<MonitorOverlay>,
    interaction: Interaction,
    mode: CaptureMode,
    window_detector: Option<WindowDetector>,
    #[cfg(target_os = "windows")]
    highlight: Option<HighlightBorder>,
  },
}

struct OverlayApp {
  gpu: Arc<GpuContext>,
  rx: mpsc::Receiver<StdinMsg>,
  state: AppState,
}

impl OverlayApp {
  fn poll_stdin(&mut self, event_loop: &ActiveEventLoop) {
    while let Ok(msg) = self.rx.try_recv() {
      match msg {
        StdinMsg::Capture(req) => self.start_capture(event_loop, req),
        StdinMsg::ShowBorder(req) => self.show_border(event_loop, req),
        StdinMsg::HideBorder => self.hide_border(),
        StdinMsg::Cancel => self.cancel_capture(),
        StdinMsg::Quit => {
          self.cancel_capture();
          event_loop.exit();
        }
      }
    }
  }

  fn start_capture(&mut self, event_loop: &ActiveEventLoop, req: CaptureRequest) {
    let t0 = std::time::Instant::now();

    if matches!(self.state, AppState::Active { .. }) {
      self.cancel_capture();
    }

    let mode = req.mode;

    // Get monitor info from xcap for per-monitor windows
    let xcap_monitors = match xcap::Monitor::all() {
      Ok(m) => m,
      Err(e) => {
        eprintln!("[overlay-daemon] failed to enumerate monitors: {e}");
        OverlayResult::Cancelled.send();
        return;
      }
    };

    // Freeze mode: capture each monitor at native resolution BEFORE creating
    // overlay windows. Uses SCK on macOS (3-5x faster than xcap), parallel
    // captures across all displays. No IPC, no temp files, no stitching.
    let mut freeze_captures: Vec<Option<image::RgbaImage>> = Vec::new();
    if mode == CaptureMode::Freeze {
      #[cfg(target_os = "macos")]
      {
        freeze_captures = capture_displays_sck(&xcap_monitors);
      }
      #[cfg(not(target_os = "macos"))]
      {
        for (i, mon) in xcap_monitors.iter().enumerate() {
          let t_cap = std::time::Instant::now();
          match mon.capture_image() {
            Ok(img) => {
              eprintln!("[daemon] captured monitor {} ({}x{}) in {:?}",
                i, img.width(), img.height(), t_cap.elapsed());
              freeze_captures.push(Some(img));
            }
            Err(e) => {
              eprintln!("[daemon] failed to capture monitor {}: {e}", i);
              freeze_captures.push(None);
            }
          }
        }
      }
      if freeze_captures.iter().all(|c| c.is_none()) {
        eprintln!("[overlay-daemon] all monitor captures failed");
        OverlayResult::Cancelled.send();
        return;
      }
    }

    let mut monitor_overlays: Vec<MonitorOverlay> = Vec::new();

    for (i, mon) in xcap_monitors.iter().enumerate() {
      let mx = mon.x().unwrap_or(0);
      let my = mon.y().unwrap_or(0);
      let mw = mon.width().unwrap_or(0);
      let mh = mon.height().unwrap_or(0);
      if mw == 0 || mh == 0 { continue; }

      eprintln!("[overlay-daemon] monitor {}: origin=({},{}) size={}x{}", i, mx, my, mw, mh);

      let win = match window::create_overlay_window(event_loop, mx, my, mw, mh, mode) {
        Ok(w) => Arc::new(w),
        Err(e) => {
          eprintln!("[overlay-daemon] window {} failed: {e}", i);
          continue;
        }
      };

      let window_scale = win.scale_factor() as f32;
      eprintln!("[overlay-daemon] monitor {} scale_factor={}", i, window_scale);

      let logical_origin_x = mx as f32;
      let logical_origin_y = my as f32;

      let (renderer, capture) = if mode == CaptureMode::Freeze {
        // Use the xcap capture we took above — native resolution, no resize
        let cap = freeze_captures.get_mut(i).and_then(|c| c.take());
        match cap {
          Some(ref img) => {
            let tex_w = img.width();
            let tex_h = img.height();
            eprintln!("[freeze] monitor {}: native capture {}x{}, surface={}x{}",
              i, tex_w, tex_h, win.inner_size().width, win.inner_size().height);

            let t_gpu = std::time::Instant::now();
            match FreezeRenderer::new(
              &self.gpu, Arc::clone(&win),
              img.as_raw(), tex_w, tex_h,
              logical_origin_x, logical_origin_y, window_scale,
            ) {
              Ok(r) => {
                eprintln!("[timing][daemon] GPU texture upload monitor {}: {:?}", i, t_gpu.elapsed());
                (ActiveRenderer::Freeze(r), cap)
              }
              Err(e) => {
                eprintln!("[overlay-daemon] freeze renderer {} failed: {e}", i);
                continue;
              }
            }
          }
          None => {
            eprintln!("[overlay-daemon] no capture for monitor {}, skipping", i);
            continue;
          }
        }
      } else {
        // Live modes
        let r = {
          #[cfg(target_os = "windows")]
          {
            let hwnd = window::get_hwnd(&win).unwrap_or(0);
            let size = win.inner_size();
            let dim_alpha = match mode {
              CaptureMode::Window | CaptureMode::SelectScreen => 20,
              _ => 77,
            };
            ActiveRenderer::Live(LiveRenderer::new(hwnd, size.width, size.height, dim_alpha, mode))
          }
          #[cfg(not(target_os = "windows"))]
          {
            let dim_alpha = match mode {
              CaptureMode::Window | CaptureMode::SelectScreen => 0.08,
              _ => 0.3,
            };
            match LiveGpuRenderer::new(&self.gpu, Arc::clone(&win), dim_alpha, logical_origin_x, logical_origin_y, window_scale) {
              Ok(r) => ActiveRenderer::LiveGpu(r),
              Err(e) => {
                eprintln!("[overlay-daemon] live GPU renderer {} failed: {e}", i);
                continue;
              }
            }
          }
        };
        (r, None)
      };

      monitor_overlays.push(MonitorOverlay {
        window: win,
        renderer,
        logical_origin_x,
        logical_origin_y,
        logical_width: mw as f32,
        logical_height: mh as f32,
        window_scale,
        freeze_capture: capture,
      });
    }

    if monitor_overlays.is_empty() {
      eprintln!("[overlay-daemon] no monitors could be initialized");
      OverlayResult::Cancelled.send();
      return;
    }

    eprintln!("[overlay-daemon] {} monitor windows created in {:?}", monitor_overlays.len(), t0.elapsed());

    let window_detector = if mode == CaptureMode::Window {
      Some(WindowDetector::new("QuickShotter"))
    } else {
      None
    };

    #[cfg(target_os = "windows")]
    let highlight_border = if mode == CaptureMode::Window {
      Some(HighlightBorder::new())
    } else {
      None
    };

    let mut interaction = Interaction::new(mode);

    if mode == CaptureMode::SelectScreen {
      let monitor_rects = get_monitor_rects(req.origin_x, req.origin_y);
      interaction.set_monitors(monitor_rects);
    }

    eprintln!("[overlay-daemon] capture active ({:?})", mode);

    self.state = AppState::Active {
      monitors: monitor_overlays,
      interaction,
      mode,
      window_detector,
      #[cfg(target_os = "windows")]
      highlight: highlight_border,
    };

    // Render first frame on all monitors, then show windows
    self.render_all_monitors();
    eprintln!("[timing][daemon] first frame rendered: {:?}", t0.elapsed());

    if let AppState::Active { monitors, .. } = &self.state {
      for mo in monitors {
        mo.window.set_visible(true);
        mo.window.focus_window();
        mo.window.request_redraw();
      }
      eprintln!("[timing][daemon] all windows visible + focused: {:?}", t0.elapsed());
    }
  }

  fn cancel_capture(&mut self) {
    if matches!(self.state, AppState::Active { .. }) {
      eprintln!("[overlay-daemon] cancelled");
      OverlayResult::Cancelled.send();
      self.state = AppState::Idle;
    }
  }

  /// Show a click-through recording border. Uses the GPU live renderer with a
  /// fixed selection rect so the shader draws the blue/red border.
  fn show_border(&mut self, event_loop: &ActiveEventLoop, req: protocol::BorderRequest) {
    // Hide any existing border
    self.hide_border();

    let win = match window::create_overlay_window(
      event_loop, req.origin_x, req.origin_y, req.bounds_w, req.bounds_h,
      CaptureMode::Live, // transparent window
    ) {
      Ok(w) => Arc::new(w),
      Err(e) => { eprintln!("[overlay-daemon] border window failed: {e}"); return; }
    };

    // Border window spans the full virtual desktop — offset is the logical origin
    let window_scale = win.scale_factor() as f32;
    let offset_x = req.origin_x as f32;
    let offset_y = req.origin_y as f32;
    let mut renderer = match LiveGpuRenderer::new(&self.gpu, Arc::clone(&win), 0.0, offset_x, offset_y, window_scale) {
      Ok(r) => r,
      Err(e) => { eprintln!("[overlay-daemon] border renderer failed: {e}"); return; }
    };

    // Create a fake interaction with a fixed selection to render the border.
    // Coords are in logical screen-space.
    let bw = 4.0; // border width + gap in logical points
    let px = req.x as f32 - bw;
    let py = req.y as f32 - bw;
    let pw = req.width as f32 + bw * 2.0;
    let ph = req.height as f32 + bw * 2.0;
    let mut interaction = Interaction::new(CaptureMode::RecordRegion);
    interaction.mouse_down(px, py);
    interaction.mouse_move(px + pw, py + ph);
    // Don't call mouse_up — keep it in Dragging phase so selection() returns the rect

    renderer.render(&self.gpu, &interaction);
    win.set_visible(true);

    // Make click-through on macOS
    #[cfg(target_os = "macos")]
    {
      use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
      if let Ok(handle) = win.window_handle() {
        if let RawWindowHandle::AppKit(h) = handle.as_raw() {
          unsafe {
            let ns_view = h.ns_view.as_ptr() as *mut objc2::runtime::AnyObject;
            let ns_window: *mut objc2::runtime::AnyObject = objc2::msg_send![ns_view, window];
            if !ns_window.is_null() {
              let _: () = objc2::msg_send![ns_window, setIgnoresMouseEvents: true];
            }
          }
        }
      }
    }

    eprintln!("[overlay-daemon] border shown: {}x{} at ({},{})", req.width, req.height, req.x, req.y);
    self.state = AppState::Border { window: win, renderer };
  }

  fn hide_border(&mut self) {
    if let AppState::Border { ref window, .. } = self.state {
      window.set_outer_position(winit::dpi::PhysicalPosition::new(-10000i32, -10000i32));
      window.set_visible(false);
      eprintln!("[overlay-daemon] border hidden");
    }
    if matches!(self.state, AppState::Border { .. }) {
      self.state = AppState::Idle;
    }
  }

  fn finish_capture(&mut self) {
    // Extract result info and freeze captures BEFORE destroying state.
    // Interaction coordinates are already in logical screen-space.
    let (result, freeze_data) = match &mut self.state {
      AppState::Active { interaction, mode, monitors, .. } => {
        if interaction.phase == Phase::Cancelled {
          (Some(OverlayResult::Cancelled), None)
        } else if interaction.phase == Phase::Done {
          let base_result = match mode {
            CaptureMode::Window => {
              interaction.window_result().map(|(l, t, r, b, shift, alt)| {
                OverlayResult::Window { left: l, top: t, right: r, bottom: b, shift, alt }
              })
            }
            CaptureMode::SelectScreen => {
              interaction.screen_result().map(|idx| {
                OverlayResult::SelectScreen { monitor_index: idx as u32 }
              })
            }
            CaptureMode::RecordRegion => {
              interaction.region_result().map(|(x1, y1, x2, y2, _, _)| {
                OverlayResult::RecordRegion { x: x1, y: y1, width: (x2 - x1) as u32, height: (y2 - y1) as u32 }
              })
            }
            _ => {
              interaction.region_result().map(|(x1, y1, x2, y2, shift, alt)| {
                eprintln!("[daemon] region result: ({},{})→({},{})", x1, y1, x2, y2);
                OverlayResult::Region {
                  x1, y1, x2, y2, shift, alt,
                  crop_path: None, crop_width: None, crop_height: None,
                }
              })
            }
          };

          // For freeze mode, extract the captures for cropping
          let freeze = if *mode == CaptureMode::Freeze {
            if let Some(region) = interaction.region_result() {
              let (x1, y1, x2, y2, _, _) = region;
              let captures: Vec<_> = monitors.iter_mut().map(|m| {
                (m.freeze_capture.take(), m.logical_origin_x, m.logical_origin_y,
                 m.logical_width, m.logical_height)
              }).collect();
              Some((x1, y1, x2, y2, captures))
            } else {
              None
            }
          } else {
            None
          };

          (base_result, freeze)
        } else {
          (None, None)
        }
      }
      AppState::Idle | AppState::Border { .. } => (None, None),
    };

    if let Some(mut result) = result {
      let t_finish = std::time::Instant::now();
      eprintln!("[timing][daemon] mouse-up → finish_capture entered");

      // For freeze mode: crop from in-memory captures and stitch if selection spans monitors
      if let Some((x1, y1, x2, y2, captures)) = freeze_data {
        let sel_l = x1.min(x2) as f32;
        let sel_t = y1.min(y2) as f32;
        let sel_r = x1.max(x2) as f32;
        let sel_b = y1.max(y2) as f32;

        // Find all monitors that overlap the selection
        let overlapping: Vec<_> = captures.iter().filter(|(cap, ox, oy, w, h)| {
          cap.is_some()
            && sel_r > *ox && sel_l < *ox + *w
            && sel_b > *oy && sel_t < *oy + *h
        }).collect();

        if !overlapping.is_empty() {
          // Determine the output pixel dimensions.
          // Use the highest scale factor among overlapping monitors for output quality.
          let max_sx = overlapping.iter().map(|(cap, _, _, lw, _)| {
            cap.as_ref().unwrap().width() as f32 / lw
          }).fold(1.0_f32, f32::max);
          let max_sy = overlapping.iter().map(|(cap, _, _, _, lh)| {
            cap.as_ref().unwrap().height() as f32 / lh
          }).fold(1.0_f32, f32::max);

          let out_w = ((sel_r - sel_l) * max_sx) as u32;
          let out_h = ((sel_b - sel_t) * max_sy) as u32;

          if overlapping.len() == 1 {
            // Single monitor — fast path, no stitching needed
            let (cap, ox, oy, lw, lh) = overlapping[0];
            let capture = cap.as_ref().unwrap();
            let sx = capture.width() as f32 / lw;
            let sy = capture.height() as f32 / lh;
            let cx = ((sel_l - ox) * sx).max(0.0) as u32;
            let cy = ((sel_t - oy) * sy).max(0.0) as u32;
            let cw = out_w.min(capture.width().saturating_sub(cx));
            let ch = out_h.min(capture.height().saturating_sub(cy));

            eprintln!("[freeze-crop] single monitor crop ({},{}) {}x{} from {}x{} (scale={:.1}x{:.1})",
              cx, cy, cw, ch, capture.width(), capture.height(), sx, sy);

            let cropped = crop_rgba(capture.as_raw(), capture.width(), capture.height(), cx, cy, cw, ch);
            let temp = std::env::temp_dir().join("qs_freeze_crop.raw");
            if std::fs::write(&temp, &cropped).is_ok() {
              eprintln!("[freeze-crop] wrote {}x{} crop ({} bytes) in {:?}",
                cw, ch, cropped.len(), t_finish.elapsed());
              if let OverlayResult::Region { ref mut crop_path, ref mut crop_width, ref mut crop_height, .. } = result {
                *crop_path = Some(temp.to_string_lossy().to_string());
                *crop_width = Some(cw);
                *crop_height = Some(ch);
              }
            }
          } else {
            // Multi-monitor — stitch overlapping portions into one output image.
            // When monitors have different scales (e.g. 1x + 2x), we nearest-neighbor
            // upscale the lower-res source to match the output canvas scale.
            eprintln!("[freeze-crop] cross-monitor stitch: {}x{} from {} monitors (output scale={:.1}x{:.1})",
              out_w, out_h, overlapping.len(), max_sx, max_sy);
            let mut canvas = vec![0u8; (out_w * out_h * 4) as usize];

            for (cap, ox, oy, lw, lh) in &overlapping {
              let capture = cap.as_ref().unwrap();
              let sx = capture.width() as f32 / lw;
              let sy = capture.height() as f32 / lh;

              // Intersection of selection with this monitor (in logical coords)
              let inter_l = sel_l.max(*ox);
              let inter_t = sel_t.max(*oy);
              let inter_r = sel_r.min(*ox + *lw);
              let inter_b = sel_b.min(*oy + *lh);

              // Source coords in this capture's pixel space
              let src_x = ((inter_l - ox) * sx) as u32;
              let src_y = ((inter_t - oy) * sy) as u32;

              // Destination region in output canvas (at max scale)
              let dst_x = ((inter_l - sel_l) * max_sx) as u32;
              let dst_y = ((inter_t - sel_t) * max_sy) as u32;
              let dst_w = ((inter_r - inter_l) * max_sx) as u32;
              let dst_h = ((inter_b - inter_t) * max_sy) as u32;

              // Scale ratio from source pixels to destination pixels
              let upscale_x = max_sx / sx;
              let upscale_y = max_sy / sy;

              eprintln!("[freeze-crop]   monitor at ({:.0},{:.0}): src ({},{}) → dst ({},{}) {}x{} upscale={:.1}x{:.1}",
                ox, oy, src_x, src_y, dst_x, dst_y, dst_w, dst_h, upscale_x, upscale_y);

              // Blit with nearest-neighbor upscaling
              let src_data = capture.as_raw();
              let cap_w = capture.width();
              for dy in 0..dst_h.min(out_h.saturating_sub(dst_y)) {
                let src_row = src_y + (dy as f32 / upscale_y) as u32;
                for dx in 0..dst_w.min(out_w.saturating_sub(dst_x)) {
                  let src_col = src_x + (dx as f32 / upscale_x) as u32;
                  let s_idx = ((src_row * cap_w + src_col) * 4) as usize;
                  let d_idx = (((dst_y + dy) * out_w + dst_x + dx) * 4) as usize;
                  if s_idx + 4 <= src_data.len() && d_idx + 4 <= canvas.len() {
                    canvas[d_idx..d_idx + 4].copy_from_slice(&src_data[s_idx..s_idx + 4]);
                  }
                }
              }
            }

            let temp = std::env::temp_dir().join("qs_freeze_crop.raw");
            if std::fs::write(&temp, &canvas).is_ok() {
              eprintln!("[freeze-crop] wrote stitched {}x{} crop ({} bytes) in {:?}",
                out_w, out_h, canvas.len(), t_finish.elapsed());
              if let OverlayResult::Region { ref mut crop_path, ref mut crop_width, ref mut crop_height, .. } = result {
                *crop_path = Some(temp.to_string_lossy().to_string());
                *crop_width = Some(out_w);
                *crop_height = Some(out_h);
              }
            }
          }
        }
      }

      self.state = AppState::Idle;
      eprintln!("[timing][daemon] windows destroyed: {:?}", t_finish.elapsed());

      // Deactivate daemon so macOS returns focus to the previous app.
      #[cfg(target_os = "macos")]
      {
        use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy};
        unsafe {
          let mtm = objc2::MainThreadMarker::new_unchecked();
          let app = NSApplication::sharedApplication(mtm);
          app.setActivationPolicy(NSApplicationActivationPolicy::Prohibited);
          app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
        }
      }
      #[cfg(target_os = "windows")]
      {
        use windows::Win32::Graphics::Dwm::DwmFlush;
        unsafe { let _ = DwmFlush(); }
      }
      eprintln!("[timing][daemon] post-destroy: {:?}", t_finish.elapsed());
      result.send();
      eprintln!("[timing][daemon] result sent to host: {:?}", t_finish.elapsed());
    }
  }

  fn render_all_monitors(&mut self) {
    if let AppState::Active { monitors, interaction, .. } = &mut self.state {
      for mo in monitors.iter_mut() {
        match &mut mo.renderer {
          ActiveRenderer::Freeze(r) => r.render(&self.gpu, interaction),
          #[cfg(target_os = "windows")]
          ActiveRenderer::Live(r) => r.render(interaction),
          ActiveRenderer::LiveGpu(r) => r.render(&self.gpu, interaction),
          ActiveRenderer::WindowMode => {}
        }
      }
    }
  }

  /// Find which monitor overlay a window ID belongs to. Returns (index, physical offset).
  fn find_monitor_for_window(&self, window_id: WindowId) -> Option<(usize, f32, f32, f32)> {
    if let AppState::Active { monitors, .. } = &self.state {
      for (i, mo) in monitors.iter().enumerate() {
        if mo.window.id() == window_id {
          return Some((i, mo.logical_origin_x, mo.logical_origin_y, mo.window_scale));
        }
      }
    }
    None
  }

  fn update_window_detection(&mut self) {
    if let AppState::Active {
      interaction, window_detector, mode,
      #[cfg(target_os = "windows")]
      highlight,
      ..
    } = &mut self.state {
      if *mode == CaptureMode::Window {
        if let Some(ref detector) = window_detector {
          // Interaction coords are already screen-space
          let screen_x = interaction.current_x as i32;
          let screen_y = interaction.current_y as i32;

          if let Some(wrect) = detector.window_at(screen_x, screen_y) {
            let local_rect = Rect {
              x1: wrect.left as f32,
              y1: wrect.top as f32,
              x2: wrect.right as f32,
              y2: wrect.bottom as f32,
            };
            interaction.set_hovered_window(Some(local_rect));
            interaction.set_selected_window(wrect.left, wrect.top, wrect.right, wrect.bottom);

            #[cfg(target_os = "windows")]
            if let Some(ref mut hl) = highlight {
              hl.show(wrect.left, wrect.top, wrect.right, wrect.bottom);
            }
          } else {
            interaction.set_hovered_window(None);
            #[cfg(target_os = "windows")]
            if let Some(ref mut hl) = highlight {
              hl.hide();
            }
          }
        }
      } else if *mode == CaptureMode::SelectScreen {
        interaction.update_hovered_monitor();
      }
    }
  }
}

/// Crop a rectangle from raw RGBA data. Returns a new buffer with just the cropped region.
fn crop_rgba(src: &[u8], src_w: u32, _src_h: u32, x: u32, y: u32, w: u32, h: u32) -> Vec<u8> {
  let mut dst = vec![0u8; (w * h * 4) as usize];
  for row in 0..h {
    let src_row = y + row;
    let src_start = ((src_row * src_w + x) * 4) as usize;
    let src_end = src_start + (w * 4) as usize;
    let dst_start = (row * w * 4) as usize;
    let dst_end = dst_start + (w * 4) as usize;
    if src_end <= src.len() && dst_end <= dst.len() {
      dst[dst_start..dst_end].copy_from_slice(&src[src_start..src_end]);
    }
  }
  dst
}

/// Capture all displays in parallel using ScreenCaptureKit (macOS only).
/// Returns a Vec aligned to the xcap monitor list — matched by logical origin.
/// SCK is 3-5x faster than xcap's CGWindowListCreateImage.
#[cfg(target_os = "macos")]
fn capture_displays_sck(xcap_monitors: &[xcap::Monitor]) -> Vec<Option<image::RgbaImage>> {
  use screencapturekit::shareable_content::SCShareableContent;
  use screencapturekit::screenshot_manager::SCScreenshotManager;
  use screencapturekit::stream::configuration::SCStreamConfiguration;
  use screencapturekit::stream::content_filter::SCContentFilter;

  // CoreGraphics display mode API to get native pixel dimensions
  #[allow(non_upper_case_globals)]
  type CGDisplayModeRef = *const std::ffi::c_void;
  extern "C" {
    fn CGDisplayCopyDisplayMode(display: u32) -> CGDisplayModeRef;
    fn CGDisplayModeGetPixelWidth(mode: CGDisplayModeRef) -> usize;
    fn CGDisplayModeGetPixelHeight(mode: CGDisplayModeRef) -> usize;
    fn CGDisplayModeRelease(mode: CGDisplayModeRef);
  }

  /// Get native pixel dimensions for a display via its current display mode.
  fn native_pixel_size(display_id: u32) -> (u32, u32) {
    unsafe {
      let mode = CGDisplayCopyDisplayMode(display_id);
      if mode.is_null() {
        return (0, 0);
      }
      let w = CGDisplayModeGetPixelWidth(mode) as u32;
      let h = CGDisplayModeGetPixelHeight(mode) as u32;
      CGDisplayModeRelease(mode);
      (w, h)
    }
  }

  let t0 = std::time::Instant::now();

  let content = match SCShareableContent::get() {
    Ok(c) => c,
    Err(e) => {
      eprintln!("[sck-daemon] SCShareableContent failed: {e}, falling back to xcap");
      return xcap_monitors.iter().enumerate().map(|(i, mon)| {
        mon.capture_image().ok().inspect(|img| {
          eprintln!("[daemon] xcap fallback monitor {} ({}x{})", i, img.width(), img.height());
        })
      }).collect();
    }
  };

  let sck_displays: Vec<_> = content.displays();

  // Build SCK captures in parallel threads
  let handles: Vec<_> = sck_displays.iter().map(|display| {
    let frame = display.frame();
    let display_id = display.display_id();
    let filter = SCContentFilter::create()
      .with_display(display)
      .with_excluding_windows(&[])
      .build();

    // Request native pixel dimensions for full-fidelity Retina capture.
    let (dw, dh) = native_pixel_size(display_id);
    // Fall back to logical if display mode query fails
    let (dw, dh) = if dw == 0 || dh == 0 {
      (frame.width as u32, frame.height as u32)
    } else {
      (dw, dh)
    };

    std::thread::spawn(move || {
      let t_cap = std::time::Instant::now();
      let config = SCStreamConfiguration::new()
        .with_width(dw)
        .with_height(dh)
        .with_shows_cursor(false);

      match SCScreenshotManager::capture_image(&filter, &config) {
        Ok(img) => {
          let rgba = img.rgba_data().ok();
          let w = img.width() as u32;
          let h = img.height() as u32;
          eprintln!("[sck-daemon] captured display ({}x{} requested, {}x{} actual) at ({:.0},{:.0}) in {:?}",
            dw, dh, w, h, frame.x, frame.y, t_cap.elapsed());
          rgba.and_then(|data| image::RgbaImage::from_raw(w, h, data))
        }
        Err(e) => {
          eprintln!("[sck-daemon] capture failed at ({:.0},{:.0}): {e}", frame.x, frame.y);
          None
        }
      }
    })
  }).collect();

  // Collect results, keyed by logical origin
  let sck_results: Vec<_> = sck_displays.iter().zip(handles).map(|(display, handle)| {
    let frame = display.frame();
    let img = handle.join().ok().flatten();
    (frame.x as i32, frame.y as i32, img)
  }).collect();

  // Match SCK results to xcap monitor ordering by logical origin
  let result: Vec<Option<image::RgbaImage>> = xcap_monitors.iter().map(|mon| {
    let mx = mon.x().unwrap_or(0);
    let my = mon.y().unwrap_or(0);
    sck_results.iter()
      .find(|(sx, sy, _)| *sx == mx && *sy == my)
      .and_then(|(_, _, img)| img.clone())
  }).collect();

  eprintln!("[sck-daemon] all {} displays captured in {:?}", result.iter().filter(|r| r.is_some()).count(), t0.elapsed());
  result
}

/// Window capture mode — standalone polling loop (runs on background thread).
#[cfg(target_os = "windows")]
fn run_window_mode_loop() -> OverlayResult {
  use windows::Win32::UI::WindowsAndMessaging::GetCursorPos;
  use windows::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;
  use windows::Win32::Foundation::POINT;

  eprintln!("[overlay-daemon] window mode: starting polling loop");

  let detector = WindowDetector::new("QuickShotter");
  let mut highlight = HighlightBorder::new();

  // Wait for all keys to be fully released (hotkey combo was just pressed)
  loop {
    let any_held = unsafe {
      GetAsyncKeyState(0x11) & 0x8000u16 as i16 != 0  // Ctrl
      || GetAsyncKeyState(0x12) & 0x8000u16 as i16 != 0  // Alt
      || GetAsyncKeyState(0x10) & 0x8000u16 as i16 != 0  // Shift
      || GetAsyncKeyState(0x57) & 0x8000u16 as i16 != 0  // W
      || GetAsyncKeyState(0x1B) & 0x8000u16 as i16 != 0  // Escape
      || GetAsyncKeyState(0x01) & 0x8000u16 as i16 != 0  // Left click
    };
    if !any_held { break; }
    std::thread::sleep(std::time::Duration::from_millis(10));
  }
  // Extra delay for good measure
  std::thread::sleep(std::time::Duration::from_millis(50));

  eprintln!("[overlay-daemon] window mode: keys released, polling active");

  let mut last_window: Option<(i32, i32, i32, i32)> = None;
  let mut click_was_down = false;

  loop {
    // Escape to cancel
    if unsafe { GetAsyncKeyState(0x1B) } & 0x8000u16 as i16 != 0 {
      highlight.hide();
      eprintln!("[overlay-daemon] window mode: cancelled via Escape");
      return OverlayResult::Cancelled;
    }

    let mut pt = POINT { x: 0, y: 0 };
    unsafe { let _ = GetCursorPos(&mut pt); }

    if let Some(wrect) = detector.window_at(pt.x, pt.y) {
      let bounds = (wrect.left, wrect.top, wrect.right, wrect.bottom);

      if last_window.as_ref() != Some(&bounds) {
        highlight.show(wrect.left, wrect.top, wrect.right, wrect.bottom);
        last_window = Some(bounds);
      }

      // Left click — edge detect
      let click_down = unsafe { GetAsyncKeyState(0x01) } & 0x8000u16 as i16 != 0;
      if click_down && !click_was_down {
        highlight.hide();
        std::thread::sleep(std::time::Duration::from_millis(50));
        let result = OverlayResult::Window {
          left: wrect.left,
          top: wrect.top,
          right: wrect.right,
          bottom: wrect.bottom,
          shift: unsafe { GetAsyncKeyState(0x10) } & 0x8000u16 as i16 != 0,
          alt: unsafe { GetAsyncKeyState(0x12) } & 0x8000u16 as i16 != 0,
        };
        eprintln!("[overlay-daemon] result: {result:?}");
        return result;
      }
      click_was_down = click_down;
    } else {
      if last_window.is_some() {
        highlight.hide();
        last_window = None;
      }
      click_was_down = unsafe { GetAsyncKeyState(0x01) } & 0x8000u16 as i16 != 0;
    }

    std::thread::sleep(std::time::Duration::from_millis(16));
  }
}

#[cfg(not(target_os = "windows"))]
fn run_window_mode_loop() -> OverlayResult {
  OverlayResult::Cancelled
}

/// Get monitor rects from xcap (screen-space coordinates for SelectScreen mode).
fn get_monitor_rects(origin_x: i32, origin_y: i32) -> Vec<MonitorRect> {
  let monitors = match xcap::Monitor::all() {
    Ok(m) => m,
    Err(_) => return Vec::new(),
  };

  monitors.iter().enumerate().filter_map(|(i, m)| {
    let x = m.x().ok()?;
    let y = m.y().ok()?;
    let w = m.width().ok()?;
    let h = m.height().ok()?;
    Some(MonitorRect {
      // Window-local coordinates (relative to virtual desktop origin)
      x: x - origin_x,
      y: y - origin_y,
      w,
      h,
      index: i,
    })
  }).collect()
}

impl ApplicationHandler<()> for OverlayApp {
  fn user_event(&mut self, event_loop: &ActiveEventLoop, _event: ()) {
    self.poll_stdin(event_loop);
  }

  fn resumed(&mut self, event_loop: &ActiveEventLoop) {
    self.poll_stdin(event_loop);
  }

  fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
    self.poll_stdin(event_loop);
  }

  fn window_event(
    &mut self,
    _event_loop: &ActiveEventLoop,
    window_id: WindowId,
    event: WindowEvent,
  ) {
    // Find which monitor this window belongs to and get its logical offset + scale
    let monitor_info = self.find_monitor_for_window(window_id);

    let mut needs_redraw = false;
    let mut needs_finish = false;

    match &mut self.state {
      AppState::Idle | AppState::Border { .. } => return,
      AppState::Active { interaction, .. } => {
        // Get the monitor's logical offset and window scale
        let (logical_ox, logical_oy, win_scale) = monitor_info
          .map(|(_, ox, oy, s)| (ox, oy, s))
          .unwrap_or((0.0, 0.0, 1.0));

        match event {
          WindowEvent::RedrawRequested => {
            needs_redraw = true;
          }

          WindowEvent::CursorMoved { position, .. } => {
            // Convert window-local physical pixels to logical screen-space.
            // Divide by window_scale to get logical window-local, then add
            // logical origin to get global logical screen coords.
            let screen_x = position.x as f32 / win_scale + logical_ox;
            let screen_y = position.y as f32 / win_scale + logical_oy;
            interaction.mouse_move(screen_x, screen_y);
            needs_redraw = true;
          }

          WindowEvent::Occluded(occluded) => {
            eprintln!("[daemon-event] Occluded({occluded})");
          }

          WindowEvent::MouseInput { state: btn_state, button, .. } => {
            eprintln!("[daemon-event] MouseInput({button:?}, {btn_state:?})");
            match (button, btn_state) {
              (MouseButton::Left, ElementState::Pressed) => {
                interaction.mouse_down(interaction.current_x, interaction.current_y);
              }
              (MouseButton::Left, ElementState::Released) => {
                interaction.mouse_up(
                  interaction.current_x, interaction.current_y,
                  interaction.shift, interaction.alt,
                );
              }
              (MouseButton::Right, ElementState::Pressed) => {
                interaction.cancel();
              }
              _ => {}
            }
            if interaction.phase == Phase::Done || interaction.phase == Phase::Cancelled {
              needs_finish = true;
            }
          }

          WindowEvent::ModifiersChanged(mods) => {
            interaction.shift = mods.state().shift_key();
            interaction.alt = mods.state().alt_key();
          }

          WindowEvent::KeyboardInput { event, .. } => {
            use winit::keyboard::{Key, NamedKey};
            if event.state == ElementState::Pressed {
              if let Key::Named(NamedKey::Escape) = event.logical_key {
                interaction.cancel();
                needs_finish = true;
              }
            }
          }

          WindowEvent::Focused(focused) => {
            eprintln!("[daemon-event] Focused({focused})");
            if !focused {
              #[cfg(not(target_os = "macos"))]
              if interaction.phase == Phase::Idle {
                interaction.cancel();
                needs_finish = true;
              }
            }
          }

          WindowEvent::CloseRequested => {
            eprintln!("[daemon-event] CloseRequested");
            interaction.cancel();
            needs_finish = true;
          }

          _ => {}
        }
      },
    }

    if needs_finish {
      self.finish_capture();
      return;
    }

    if needs_redraw {
      self.update_window_detection();
      self.render_all_monitors();
      if let AppState::Active { monitors, .. } = &self.state {
        for mo in monitors {
          mo.window.request_redraw();
        }
      }
    }
  }
}
