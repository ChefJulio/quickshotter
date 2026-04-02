//! QuickShotter native overlay daemon.
//!
//! Two rendering paths:
//! - Freeze mode: opaque wgpu window, screenshot texture on GPU
//! - Live/Window/SelectScreen/etc: Win32 layered window, CPU-rendered per-pixel alpha

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
  Cancel,
  Quit,
}

fn main() {
  // macOS: ensure this process can become the active application and receive
  // events. Without this, a child process's windows won't get mouse/keyboard
  // events because macOS doesn't deliver events to background processes.
  #[cfg(target_os = "macos")]
  {
    use objc2::msg_send;
    use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy};
    unsafe {
      let mtm = objc2::MainThreadMarker::new_unchecked();
      let app = NSApplication::sharedApplication(mtm);
      // Start as Regular to initialize the event loop, then switch to
      // Accessory so we don't appear in Cmd+Tab / Dock.
      app.setActivationPolicy(NSApplicationActivationPolicy::Regular);
      app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
      // Force-activate so we still get events despite being Accessory
      let _: () = msg_send![&*app, activateIgnoringOtherApps: true];
    }
    eprintln!("[daemon] NSApp activated (Accessory + forced)");
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

// ── App state machine ──────────────────────────────────────────────

enum AppState {
  Idle,
  Active {
    window: Arc<winit::window::Window>,
    renderer: ActiveRenderer,
    interaction: Interaction,
    mode: CaptureMode,
    origin_x: i32,
    origin_y: i32,
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
    let origin_x = req.origin_x;
    let origin_y = req.origin_y;

    let win = match window::create_overlay_window(
      event_loop, req.origin_x, req.origin_y, req.bounds_w, req.bounds_h, mode,
    ) {
      Ok(w) => Arc::new(w),
      Err(e) => {
        eprintln!("[overlay-daemon] window failed: {e}");
        OverlayResult::Cancelled.send();
        return;
      }
    };
    eprintln!("[overlay-daemon] window created in {:?}", t0.elapsed());

    let renderer = if mode == CaptureMode::Freeze {
      let screenshot_data = req.image.as_ref().and_then(|path| {
        let t_read = std::time::Instant::now();
        let data = std::fs::read(path).ok();
        eprintln!("[timing][daemon] screenshot file read ({} bytes): {:?}",
          data.as_ref().map_or(0, |d| d.len()), t_read.elapsed());
        data
      });
      match screenshot_data {
        Some(data) => {
          let t_gpu = std::time::Instant::now();
          match FreezeRenderer::new(
            &self.gpu, Arc::clone(&win),
            &data, req.image_width, req.image_height,
          ) {
            Ok(r) => {
              eprintln!("[timing][daemon] GPU texture upload + surface: {:?}", t_gpu.elapsed());
              ActiveRenderer::Freeze(r)
            }
            Err(e) => {
              eprintln!("[overlay-daemon] freeze renderer failed: {e}");
              OverlayResult::Cancelled.send();
              return;
            }
          }
        }
        None => {
          eprintln!("[overlay-daemon] no screenshot data for freeze mode");
          OverlayResult::Cancelled.send();
          return;
        }
      }
    } else {
      // Live modes: platform-specific rendering.
      // Windows: Win32 layered window with per-pixel alpha via UpdateLayeredWindow.
      // macOS: GPU-based transparent wgpu surface with Metal backend.
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
        match LiveGpuRenderer::new(&self.gpu, Arc::clone(&win), dim_alpha) {
          Ok(r) => ActiveRenderer::LiveGpu(r),
          Err(e) => {
            eprintln!("[overlay-daemon] live GPU renderer failed: {e}");
            OverlayResult::Cancelled.send();
            return;
          }
        }
      }
    };

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
      let monitors = get_monitor_rects(origin_x, origin_y);
      interaction.set_monitors(monitors);
    }

    eprintln!("[overlay-daemon] renderer ready in {:?}", t0.elapsed());
    eprintln!("[overlay-daemon] capture active ({:?})", mode);

    self.state = AppState::Active {
      window: win,
      renderer,
      interaction,
      mode,
      origin_x,
      origin_y,
      window_detector,
      #[cfg(target_os = "windows")]
      highlight: highlight_border,
    };

    // GPU-rendered modes: render first frame then show window (no white flash).
    // On Windows, live modes use layered windows that are already visible.
    let needs_gpu_show = mode == CaptureMode::Freeze
      || matches!(self.state, AppState::Active { renderer: ActiveRenderer::LiveGpu(_), .. });
    if needs_gpu_show {
      self.render_frame();
      eprintln!("[timing][daemon] first frame rendered: {:?}", t0.elapsed());
      if let AppState::Active { window, .. } = &self.state {
        window.set_visible(true);
        window.focus_window();
        window.request_redraw();
        eprintln!("[timing][daemon] window visible + focused: {:?}", t0.elapsed());
      }
    }
  }

  fn cancel_capture(&mut self) {
    if matches!(self.state, AppState::Active { .. }) {
      eprintln!("[overlay-daemon] cancelled");
      OverlayResult::Cancelled.send();
      self.state = AppState::Idle;
    }
  }

  fn finish_capture(&mut self) {
    let result = match &self.state {
      AppState::Active { interaction, mode, origin_x, origin_y, .. } => {
        if interaction.phase == Phase::Cancelled {
          Some(OverlayResult::Cancelled)
        } else if interaction.phase == Phase::Done {
          match mode {
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
              interaction.region_result(*origin_x, *origin_y).map(|(x1, y1, x2, y2, _, _)| {
                OverlayResult::RecordRegion { x: x1, y: y1, width: (x2 - x1) as u32, height: (y2 - y1) as u32 }
              })
            }
            _ => {
              interaction.region_result(*origin_x, *origin_y).map(|(x1, y1, x2, y2, shift, alt)| {
                OverlayResult::Region { x1, y1, x2, y2, shift, alt }
              })
            }
          }
        } else {
          None
        }
      }
      AppState::Idle => None,
    };

    if let Some(result) = result {
      let t_finish = std::time::Instant::now();
      eprintln!("[timing][daemon] mouse-up → finish_capture entered");
      self.state = AppState::Idle;
      eprintln!("[timing][daemon] window destroyed: {:?}", t_finish.elapsed());
      #[cfg(target_os = "windows")]
      {
        use windows::Win32::Graphics::Dwm::DwmFlush;
        unsafe { let _ = DwmFlush(); }
      }
      #[cfg(not(target_os = "windows"))]
      std::thread::sleep(std::time::Duration::from_millis(16));
      eprintln!("[timing][daemon] compositor wait done: {:?}", t_finish.elapsed());
      result.send();
      eprintln!("[timing][daemon] result sent to host: {:?}", t_finish.elapsed());
    }
  }

  fn render_frame(&mut self) {
    if let AppState::Active { renderer, interaction, .. } = &mut self.state {
      match renderer {
        ActiveRenderer::Freeze(r) => r.render(&self.gpu, interaction),
        #[cfg(target_os = "windows")]
        ActiveRenderer::Live(r) => r.render(interaction),
        ActiveRenderer::LiveGpu(r) => r.render(&self.gpu, interaction),
        ActiveRenderer::WindowMode => {} // highlight handled by update_window_detection
      }
    }
  }

  fn update_window_detection(&mut self) {
    if let AppState::Active {
      interaction, window_detector, origin_x, origin_y, mode,
      #[cfg(target_os = "windows")]
      highlight,
      ..
    } = &mut self.state {
      if *mode == CaptureMode::Window {
        if let Some(ref detector) = window_detector {
          let screen_x = interaction.current_x as i32 + *origin_x;
          let screen_y = interaction.current_y as i32 + *origin_y;

          if let Some(wrect) = detector.window_at(screen_x, screen_y) {
            let local_rect = Rect {
              x1: (wrect.left - *origin_x) as f32,
              y1: (wrect.top - *origin_y) as f32,
              x2: (wrect.right - *origin_x) as f32,
              y2: (wrect.bottom - *origin_y) as f32,
            };
            interaction.set_hovered_window(Some(local_rect));
            interaction.set_selected_window(wrect.left, wrect.top, wrect.right, wrect.bottom);

            // Show highlight border at screen-space coordinates
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

/// Get monitor rects from xcap, converted to window-local coordinates.
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
      // Window-local coordinates
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
    event_loop: &ActiveEventLoop,
    _window_id: winit::window::WindowId,
    event: WindowEvent,
  ) {
    // Process the event — extract what we need without holding a borrow on self.state
    let mut needs_redraw = false;
    let mut needs_finish = false;

    match &mut self.state {
      AppState::Idle => return,
      AppState::Active { window, interaction, .. } => match event {
        WindowEvent::RedrawRequested => {
          // Handled below after releasing the borrow
          needs_redraw = true;
        }

        WindowEvent::CursorMoved { position, .. } => {
          interaction.mouse_move(position.x as f32, position.y as f32);
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
          // On macOS, borderless overlay windows may not receive focus
          // immediately, causing a spurious blur event. Only cancel if
          // the window has been focused at least once (i.e. user tabbed away).
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
      },
    }

    // Now handle actions that need full &mut self access
    if needs_finish {
      self.finish_capture();
      return;
    }

    if needs_redraw {
      self.update_window_detection();
      self.render_frame();
      if let AppState::Active { window, .. } = &self.state {
        window.request_redraw();
      }
    }
  }
}
