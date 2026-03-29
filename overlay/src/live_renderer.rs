//! Live mode renderer — Win32 layered window with per-pixel alpha.
//!
//! Supports three visual modes:
//! - Region: dark overlay with clear cutout + blue border
//! - Window: nearly invisible overlay with blue highlight around hovered window
//! - SelectScreen: dark overlay with monitor outlines and highlight

#[cfg(target_os = "windows")]
use windows::Win32::Foundation::{HWND, POINT, SIZE};
#[cfg(target_os = "windows")]
use windows::Win32::Graphics::Gdi::{
  CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, SelectObject,
  BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS,
};
#[cfg(target_os = "windows")]
use windows::Win32::UI::WindowsAndMessaging::{UpdateLayeredWindow, ULW_ALPHA};

use crate::interaction::Interaction;
use crate::protocol::CaptureMode;

#[inline(always)]
fn bgra(b: u8, g: u8, r: u8, a: u8) -> u32 {
  u32::from_le_bytes([b, g, r, a])
}

pub struct LiveRenderer {
  width: u32,
  height: u32,
  hwnd: isize,
  buffer: Vec<u32>,
  dark_pixel: u32,
  /// Fully transparent (region mode cutout — shows live desktop).
  clear_pixel: u32,
  /// Nearly transparent but still catches mouse events (window mode highlight).
  highlight_pixel: u32,
  border_pixel: u32,
  mode: CaptureMode,
  prev_dirty: Option<(i32, i32, i32, i32)>,
}

impl LiveRenderer {
  pub fn new(hwnd: isize, width: u32, height: u32, dim_alpha: u8, mode: CaptureMode) -> Self {
    let pixel_count = (width as usize) * (height as usize);
    let dark_pixel = bgra(0, 0, 0, dim_alpha);
    let clear_pixel = bgra(0, 0, 0, 0);
    // Alpha=1: invisible to the eye but Win32 still delivers mouse events
    let highlight_pixel = bgra(0, 0, 0, 1);
    let border_pixel = bgra(0xff, 0xaa, 0x00, 0xff); // #00aaff

    let buffer = vec![dark_pixel; pixel_count];

    let mut renderer = Self {
      width, height, hwnd, buffer,
      dark_pixel, clear_pixel, highlight_pixel, border_pixel, mode,
      prev_dirty: None,
    };

    renderer.blit_to_window();
    renderer
  }

  pub fn render(&mut self, interaction: &Interaction) {
    match self.mode {
      CaptureMode::Window => self.render_window_mode(interaction),
      CaptureMode::SelectScreen => self.render_select_screen_mode(interaction),
      _ => self.render_region_mode(interaction),
    }
    self.blit_to_window();
  }

  // ── Region mode ──────────────────────────────────────

  fn render_region_mode(&mut self, interaction: &Interaction) {
    let sel = interaction.selection();
    let new_sel = sel.map(|(x1, y1, x2, y2)| (x1 as i32, y1 as i32, x2 as i32, y2 as i32));

    // Restore previous area
    if let Some((px1, py1, px2, py2)) = self.prev_dirty {
      self.fill_rect(px1, py1, px2, py2, self.dark_pixel);
    }

    // Draw new selection
    if let Some((x1, y1, x2, y2)) = new_sel {
      self.fill_rect(x1, y1, x2, y2, self.clear_pixel);
      self.draw_border(x1, y1, x2, y2, 2, self.border_pixel);
    }

    self.prev_dirty = new_sel;
  }

  // ── Window mode ──────────────────────────────────────

  fn render_window_mode(&mut self, interaction: &Interaction) {
    let bw = 3;

    // Restore previous highlighted area back to dark
    if let Some((px1, py1, px2, py2)) = self.prev_dirty {
      self.fill_rect(px1, py1, px2, py2, self.dark_pixel);
    }

    // Draw highlight around hovered window
    if let Some(ref rect) = interaction.hovered_window {
      let x1 = rect.x1 as i32;
      let y1 = rect.y1 as i32;
      let x2 = rect.x2 as i32;
      let y2 = rect.y2 as i32;

      // Use highlight_pixel (alpha=1) inside — nearly invisible but still
      // receives mouse events. Alpha=0 would pass clicks through to the
      // window below, preventing us from detecting the click.
      self.fill_rect(x1, y1, x2, y2, self.highlight_pixel);
      // Blue border
      self.draw_border(x1, y1, x2, y2, bw, self.border_pixel);

      self.prev_dirty = Some((x1, y1, x2, y2));
    } else {
      self.prev_dirty = None;
    }
  }

  // ── Select screen mode ───────────────────────────────

  fn render_select_screen_mode(&mut self, interaction: &Interaction) {
    let bw = 3;

    // Restore previous highlighted area
    if let Some((px1, py1, px2, py2)) = self.prev_dirty {
      self.fill_rect(px1, py1, px2, py2, self.dark_pixel);
    }

    // Draw thin gray outlines for all monitors
    let outline_pixel = bgra(0x80, 0x80, 0x80, 0x80);
    for mon in &interaction.monitors {
      let mx1 = mon.x as i32;
      let my1 = mon.y as i32;
      let mx2 = mx1 + mon.w as i32;
      let my2 = my1 + mon.h as i32;
      self.draw_border(mx1, my1, mx2, my2, 1, outline_pixel);
    }

    // Highlight hovered monitor (same pattern as window mode)
    if let Some(idx) = interaction.hovered_monitor {
      if let Some(mon) = interaction.monitors.get(idx) {
        let mx1 = mon.x as i32;
        let my1 = mon.y as i32;
        let mx2 = mx1 + mon.w as i32;
        let my2 = my1 + mon.h as i32;

        // highlight_pixel (alpha=1): nearly invisible but catches mouse events
        self.fill_rect(mx1, my1, mx2, my2, self.highlight_pixel);
        self.draw_border(mx1, my1, mx2, my2, bw, self.border_pixel);

        self.prev_dirty = Some((mx1, my1, mx2, my2));
      }
    } else {
      self.prev_dirty = None;
    }
  }

  // ── Drawing primitives ───────────────────────────────

  fn fill_rect(&mut self, x1: i32, y1: i32, x2: i32, y2: i32, pixel: u32) {
    let w = self.width as i32;
    let h = self.height as i32;
    let x1 = x1.max(0) as usize;
    let y1 = y1.max(0) as usize;
    let x2 = x2.min(w) as usize;
    let y2 = y2.min(h) as usize;
    let stride = self.width as usize;

    for y in y1..y2 {
      let row_start = y * stride + x1;
      let row_end = y * stride + x2;
      if row_end <= self.buffer.len() {
        self.buffer[row_start..row_end].fill(pixel);
      }
    }
  }

  /// Draw border INSIDE the rect (so it's visible even for fullscreen rects).
  fn draw_border(&mut self, x1: i32, y1: i32, x2: i32, y2: i32, bw: i32, pixel: u32) {
    self.fill_rect(x1, y1, x2, y1 + bw, pixel);       // top
    self.fill_rect(x1, y2 - bw, x2, y2, pixel);       // bottom
    self.fill_rect(x1, y1 + bw, x1 + bw, y2 - bw, pixel); // left
    self.fill_rect(x2 - bw, y1 + bw, x2, y2 - bw, pixel); // right
  }

  #[cfg(target_os = "windows")]
  fn blit_to_window(&self) {
    unsafe {
      let hwnd = HWND(self.hwnd as *mut _);
      let hdc_mem = CreateCompatibleDC(None);
      if hdc_mem.0.is_null() { return; }

      let bmi = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
          biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
          biWidth: self.width as i32,
          biHeight: -(self.height as i32),
          biPlanes: 1,
          biBitCount: 32,
          biCompression: BI_RGB.0,
          ..Default::default()
        },
        bmiColors: [Default::default()],
      };

      let mut bits_ptr: *mut std::ffi::c_void = std::ptr::null_mut();
      let dib = CreateDIBSection(
        Some(hdc_mem), &bmi, DIB_RGB_COLORS, &mut bits_ptr, None, 0,
      );

      if let Ok(dib) = dib {
        let byte_count = self.buffer.len() * 4;
        std::ptr::copy_nonoverlapping(
          self.buffer.as_ptr() as *const u8,
          bits_ptr as *mut u8,
          byte_count,
        );

        let old = SelectObject(hdc_mem, dib.into());

        let size = SIZE { cx: self.width as i32, cy: self.height as i32 };
        let src_point = POINT { x: 0, y: 0 };
        let blend = windows::Win32::Graphics::Gdi::BLENDFUNCTION {
          BlendOp: 0,
          BlendFlags: 0,
          SourceConstantAlpha: 255,
          AlphaFormat: 1,
        };

        let _ = UpdateLayeredWindow(
          hwnd, None, None, Some(&size),
          Some(hdc_mem), Some(&src_point),
          Default::default(), Some(&blend), ULW_ALPHA,
        );

        SelectObject(hdc_mem, old);
        let _ = DeleteObject(dib.into());
      }

      let _ = DeleteDC(hdc_mem);
    }
  }

  #[cfg(not(target_os = "windows"))]
  fn blit_to_window(&self) {}
}
