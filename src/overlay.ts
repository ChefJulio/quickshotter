import './overlay.css';
import { invoke } from '@tauri-apps/api/core';
import { getCurrentWindow } from '@tauri-apps/api/window';
import type { WindowRect } from './types';

let canvas: HTMLCanvasElement;
let ctx: CanvasRenderingContext2D;

// Freeze/window mode only
let screenshotImage: HTMLImageElement | null = null;
let dimmedCanvas: HTMLCanvasElement;
let dimmedCtx: CanvasRenderingContext2D;

// Region selection state
let isDragging = false;
let startX = 0;
let startY = 0;
let currentX = 0;
let currentY = 0;

let mode: string = 'instant';
let cancelled = false;
let isRecordingActive = false; // Set when recording starts; prevents blur/cancel

// DPI scale factor: CSS pixels * scale = physical pixels (what Rust uses).
// In freeze/window mode, derived from the actual screenshot dimensions
// for correct mixed-DPI multi-monitor mapping. Falls back to devicePixelRatio.
let captureScale: number | null = null;
function scale(): number {
  return captureScale ?? (window.devicePixelRatio || 1);
}

// Window capture state
let highlightRect: { x: number; y: number; w: number; h: number } | null = null;
// Absolute screen coords from Rust, sent back directly for capture
let highlightPhysical: { left: number; top: number; right: number; bottom: number } | null = null;
let windowPollPending = false;
// Virtual desktop origin -- screen coords from Rust are absolute, so we subtract
// this to get overlay-local coordinates before converting to CSS pixels.
let overlayOriginX = 0;
let overlayOriginY = 0;
// On macOS, xcap returns logical points that match CSS pixels directly (scale = 1).
// On Windows, xcap returns physical pixels requiring devicePixelRatio conversion.
const xcapScale = /Mac/.test(navigator.platform) ? 1 : (window.devicePixelRatio || 1);

function initCanvas() {
  canvas = document.getElementById('overlay-canvas') as HTMLCanvasElement;
  // Hint browser to use GPU-backed canvas -- we never call getImageData
  ctx = canvas.getContext('2d', { willReadFrequently: false })!;
  // Canvas buffer at CSS pixel resolution; all drawing uses CSS coords
  canvas.width = window.innerWidth;
  canvas.height = window.innerHeight;

  // Pre-create dimmed canvas (used in freeze/window mode)
  dimmedCanvas = document.createElement('canvas');
  dimmedCtx = dimmedCanvas.getContext('2d', { willReadFrequently: false })!;

  canvas.addEventListener('mousedown', onMouseDown);
  canvas.addEventListener('mousemove', onMouseMove);
  canvas.addEventListener('mouseup', onMouseUp);
  canvas.addEventListener('contextmenu', (e) => {
    e.preventDefault();
    cancel();
  });

  document.addEventListener('keydown', onKeyDown);
}

function showInstantOverlay() {
  ctx.fillStyle = 'rgba(0, 0, 0, 0.3)';
  ctx.fillRect(0, 0, canvas.width, canvas.height);
}

function showOcrOverlay() {
  ctx.fillStyle = 'rgba(0, 0, 0, 0.3)';
  ctx.fillRect(0, 0, canvas.width, canvas.height);
  ctx.font = 'bold 16px Consolas, monospace';
  ctx.fillStyle = 'rgba(0, 200, 120, 0.9)';
  ctx.textAlign = 'center';
  ctx.fillText('Select text to recognize', canvas.width / 2, 40);
  ctx.textAlign = 'start';
}


function showRecordRegionOverlay() {
  ctx.fillStyle = 'rgba(0, 0, 0, 0.3)';
  ctx.fillRect(0, 0, canvas.width, canvas.height);
  // Show hint text
  ctx.font = 'bold 16px Consolas, monospace';
  ctx.fillStyle = 'rgba(255, 60, 60, 0.9)';
  ctx.textAlign = 'center';
  ctx.fillText('Select region to record', canvas.width / 2, 40);
  ctx.textAlign = 'start';
}

function showRecordingBoundary(x: number, y: number, w: number, h: number) {
  ctx.clearRect(0, 0, canvas.width, canvas.height);
  // Red border OUTSIDE the recording region so it doesn't get captured.
  // strokeRect draws the stroke centered on the coordinates, so offset by
  // half the line width to keep the inner edge just outside the capture area.
  const bw = 3;
  ctx.strokeStyle = 'rgba(255, 60, 60, 0.9)';
  ctx.lineWidth = bw;
  ctx.setLineDash([8, 4]);
  const offset = Math.ceil(bw / 2) + 1; // extra 1px gap from capture edge
  ctx.strokeRect(x - offset, y - offset, w + offset * 2, h + offset * 2);
  ctx.setLineDash([]);
  // Small "REC" label above the region
  ctx.font = 'bold 12px Consolas, monospace';
  ctx.fillStyle = 'rgba(255, 60, 60, 0.95)';
  const labelY = y > 26 ? y - offset - 5 : y + h + offset + 16;
  ctx.fillText('REC', x, labelY);
}

// -- Select screen mode --

interface MonitorInfo {
  index: number;
  x: number;
  y: number;
  width: number;
  height: number;
}

let monitors: MonitorInfo[] = [];
let hoveredMonitor: number | null = null;

function showSelectScreenOverlay() {
  ctx.fillStyle = 'rgba(0, 0, 0, 0.4)';
  ctx.fillRect(0, 0, canvas.width, canvas.height);
  canvas.style.cursor = 'pointer';
  drawMonitorLabels();
}

function drawMonitorLabels() {
  const s = xcapScale;
  for (let i = 0; i < monitors.length; i++) {
    const m = monitors[i];
    // Convert absolute screen coords to overlay-local CSS coords
    const cx = (m.x - overlayOriginX) / s;
    const cy = (m.y - overlayOriginY) / s;
    const cw = m.width / s;
    const ch = m.height / s;

    if (hoveredMonitor === i) {
      // Bright highlight on hovered monitor
      ctx.fillStyle = 'rgba(0, 170, 255, 0.15)';
      ctx.fillRect(cx, cy, cw, ch);
      ctx.strokeStyle = '#00aaff';
      ctx.lineWidth = 3;
      ctx.strokeRect(cx + 1.5, cy + 1.5, cw - 3, ch - 3);
    } else {
      ctx.strokeStyle = 'rgba(255, 255, 255, 0.3)';
      ctx.lineWidth = 1;
      ctx.strokeRect(cx + 0.5, cy + 0.5, cw - 1, ch - 1);
    }

    // Label
    const label = `Screen ${i + 1}`;
    const sub = `${m.width} x ${m.height}`;
    ctx.font = 'bold 24px Consolas, monospace';
    ctx.fillStyle = hoveredMonitor === i ? '#00aaff' : 'rgba(255, 255, 255, 0.7)';
    ctx.textAlign = 'center';
    ctx.fillText(label, cx + cw / 2, cy + ch / 2 - 8);
    ctx.font = '14px Consolas, monospace';
    ctx.fillStyle = 'rgba(255, 255, 255, 0.5)';
    ctx.fillText(sub, cx + cw / 2, cy + ch / 2 + 16);
    ctx.textAlign = 'start';
  }
}

function onMouseMoveSelectScreen(e: MouseEvent) {
  const s = xcapScale;
  const prev = hoveredMonitor;
  hoveredMonitor = null;
  for (let i = 0; i < monitors.length; i++) {
    const m = monitors[i];
    const cx = (m.x - overlayOriginX) / s;
    const cy = (m.y - overlayOriginY) / s;
    const cw = m.width / s;
    const ch = m.height / s;
    if (e.clientX >= cx && e.clientX < cx + cw && e.clientY >= cy && e.clientY < cy + ch) {
      hoveredMonitor = i;
      break;
    }
  }
  if (hoveredMonitor !== prev) {
    ctx.clearRect(0, 0, canvas.width, canvas.height);
    ctx.fillStyle = 'rgba(0, 0, 0, 0.4)';
    ctx.fillRect(0, 0, canvas.width, canvas.height);
    drawMonitorLabels();
  }
}

function onMouseDownSelectScreen(e: MouseEvent) {
  if (e.button === 2) { cancel(); return; }
  if (e.button !== 0 || hoveredMonitor === null) return;
  const idx = hoveredMonitor;
  const m = monitors[idx];
  const s = xcapScale;
  const cssX1 = (m.x - overlayOriginX) / s;
  const cssY1 = (m.y - overlayOriginY) / s;
  const cssX2 = cssX1 + m.width / s;
  const cssY2 = cssY1 + m.height / s;
  captureWithDelay(cssX1, cssY1, cssX2, cssY2,
    { mode: 'select_screen', monitorIndex: idx },
    () => invoke('complete_select_screen_capture', { monitorIndex: idx })
  );
}

function showWindowOverlay() {
  // Nearly invisible fill -- just enough to capture mouse events on Windows.
  // The user sees through to the real desktop.
  ctx.fillStyle = 'rgba(0, 0, 0, 0.01)';
  ctx.fillRect(0, 0, canvas.width, canvas.height);
  canvas.style.cursor = 'pointer';
}

function loadScreenshot(base64Data: string) {
  const img = new Image();
  img.onload = () => {
    screenshotImage = img;

    // Derive scale from actual screenshot dimensions vs CSS canvas.
    // This correctly handles mixed-DPI multi-monitor setups where
    // devicePixelRatio alone would be wrong.
    captureScale = img.naturalWidth / canvas.width;

    // Keep canvas at CSS pixel resolution; draw the physical-pixel screenshot
    // scaled to fit so everything uses a single CSS coordinate space.
    dimmedCanvas.width = canvas.width;
    dimmedCanvas.height = canvas.height;

    // Draw screenshot scaled to CSS dimensions
    dimmedCtx.drawImage(img, 0, 0, canvas.width, canvas.height);
    dimmedCtx.fillStyle = 'rgba(0, 0, 0, 0.45)';
    dimmedCtx.fillRect(0, 0, canvas.width, canvas.height);

    ctx.drawImage(dimmedCanvas, 0, 0);

    // Window mode: set cursor
    if (mode === 'window') {
      canvas.style.cursor = 'pointer';
    }
  };
  img.src = `data:image/jpeg;base64,${base64Data}`;
}

// -- Region capture handlers --

function onMouseDown(e: MouseEvent) {
  // Right-click cancels (contextmenu may not fire during an active drag)
  if (e.button === 2) {
    e.preventDefault();
    cancel();
    return;
  }
  if (e.button !== 0) return;

  if (mode === 'select_screen') {
    onMouseDownSelectScreen(e);
    return;
  }

  if (mode === 'window') {
    onMouseDownWindow(e.shiftKey, e.altKey);
    return;
  }

  isDragging = true;
  startX = e.clientX;
  startY = e.clientY;
  currentX = e.clientX;
  currentY = e.clientY;
}

function onMouseMove(e: MouseEvent) {
  if (mode === 'select_screen') {
    onMouseMoveSelectScreen(e);
    return;
  }

  if (mode === 'window') {
    onMouseMoveWindow();
    return;
  }

  if (!isDragging) return;
  currentX = e.clientX;
  currentY = e.clientY;
  drawSelection();
}

async function onMouseUp(e: MouseEvent) {
  if (mode === 'window') return; // Window mode uses click, not drag

  if (!isDragging) return;
  isDragging = false;

  const cssX1 = Math.min(startX, e.clientX);
  const cssY1 = Math.min(startY, e.clientY);
  const cssX2 = Math.max(startX, e.clientX);
  const cssY2 = Math.max(startY, e.clientY);

  if (cssX2 - cssX1 < 3 || cssY2 - cssY1 < 3) {
    cancel();
    return;
  }

  // Convert CSS coords to physical pixels for Rust
  const x1 = Math.round(cssX1 * scale());
  const y1 = Math.round(cssY1 * scale());
  const x2 = Math.round(cssX2 * scale());
  const y2 = Math.round(cssY2 * scale());

  if (mode === 'record_region') {
    // Region recording: use xcapScale for coordinates, NOT scale()/DPR.
    // Recording region coords must be in xcap's coordinate space:
    //   macOS: logical points (xcapScale=1) -- monitor dims are logical
    //   Windows: physical pixels (xcapScale=DPR) -- monitor dims are physical
    const rx1 = Math.round(cssX1 * xcapScale);
    const ry1 = Math.round(cssY1 * xcapScale);
    const rx2 = Math.round(cssX2 * xcapScale);
    const ry2 = Math.round(cssY2 * xcapScale);
    const x = Math.min(rx1, rx2);
    const y = Math.min(ry1, ry2);
    const width = Math.abs(rx2 - rx1);
    const height = Math.abs(ry2 - ry1);
    console.log('record_region coords:', {
      css: { x1: cssX1, y1: cssY1, x2: cssX2, y2: cssY2 },
      xcap: { x, y, width, height },
      xcapScale, dpr: window.devicePixelRatio,
      canvas: { w: canvas.width, h: canvas.height },
      innerSize: { w: window.innerWidth, h: window.innerHeight },
    });
    try {
      // Set flag BEFORE the invoke -- Rust opens the recording indicator window
      // which steals focus, firing the blur handler. The flag must already be
      // true so the blur handler skips the cancel().
      isRecordingActive = true;

      // Clear the canvas completely and wait for the browser to repaint so
      // the selection UI (dim + blue rectangle) doesn't appear in the first frame.
      ctx.clearRect(0, 0, canvas.width, canvas.height);
      await new Promise<void>(r => requestAnimationFrame(() => setTimeout(r, 50)));

      // Pass CSS coords so Rust can position the indicator at the selection
      await invoke('start_region_recording', {
        x, y, width, height,
        indicatorX: Math.round(cssX1),
        indicatorY: Math.round(cssY1),
      });

      // Now draw the recording boundary (outside capture region, won't be captured)
      showRecordingBoundary(cssX1, cssY1, cssX2 - cssX1, cssY2 - cssY1);

      // Make overlay click-through so the user can interact with everything.
      // Remove all canvas listeners first to prevent JS-level event blocking.
      canvas.removeEventListener('mousedown', onMouseDown);
      canvas.removeEventListener('mousemove', onMouseMove);
      canvas.removeEventListener('mouseup', onMouseUp);
      document.removeEventListener('keydown', onKeyDown);

      const win = getCurrentWindow();
      await win.setIgnoreCursorEvents(true).catch(() => {
        // If click-through fails, log it but keep overlay. User can still
        // stop recording via Escape on the indicator or the Stop button.
        console.warn('setIgnoreCursorEvents failed - overlay may block clicks');
      });

      // Poll recording state; close overlay when recording stops
      const poll = setInterval(async () => {
        try {
          const state: { is_recording: boolean } = await invoke('get_recording_state');
          if (!state.is_recording) {
            clearInterval(poll);
            isRecordingActive = false;
            cancelled = false;
            cancel();
          }
        } catch {
          clearInterval(poll);
          isRecordingActive = false;
          cancelled = false;
          cancel();
        }
      }, 500);
    } catch (e) {
      console.error('Region recording failed:', e);
      isRecordingActive = false;
      cancel();
    }
    return;
  } else if (mode === 'ocr') {
    captureWithDelay(cssX1, cssY1, cssX2, cssY2,
      { mode: 'ocr', x1, y1, x2, y2 },
      () => invoke('complete_ocr_capture', { x1, y1, x2, y2 })
    );
  } else {
    const shiftKey = e.shiftKey;
    const altKey = e.altKey;
    captureWithDelay(cssX1, cssY1, cssX2, cssY2,
      { mode: 'region', x1, y1, x2, y2, forceAnnotate: shiftKey, toggleSave: altKey },
      () => invoke('complete_region_capture', { x1, y1, x2, y2, forceAnnotate: shiftKey, toggleSave: altKey })
    );
  }
}

// -- Countdown delay before capture --

async function captureWithDelay(
  cssX1: number, cssY1: number, _cssX2: number, _cssY2: number,
  captureParams: Record<string, unknown>,
  captureAction: () => Promise<unknown>,
) {
  let delay: number;
  try {
    delay = await invoke<number>('get_capture_delay');
  } catch {
    delay = 0;
  }

  if (delay <= 0) {
    captureAction().catch((e) => {
      console.error('Capture failed:', e);
      cancel();
    });
    return;
  }

  // Convert selection CSS coords to physical screen coords.
  // All coordinates sent to Rust are now in physical screen pixels.
  const s = xcapScale; // DPR on Windows, 1 on macOS
  const toPhysX = (css: number) => Math.round(css * s + overlayOriginX);
  const toPhysY = (css: number) => Math.round(css * s + overlayOriginY);
  const toPhysW = (css: number) => Math.round(css * s);

  // Position countdown above the selection; if no room, try below; last resort: inside.
  const countdownSize = 80;
  const pad = 8;
  const selH = _cssY2 - cssY1;
  let posX = Math.max(pad, cssX1 - pad);
  let posY = cssY1 - countdownSize - pad;
  if (posY < 0) {
    const belowY = cssY1 + selH + pad;
    if (belowY + countdownSize <= window.innerHeight) {
      posY = belowY;
    } else {
      posY = pad;
    }
  }

  invoke('prepare_delayed_capture', {
    params: captureParams,
    posX: toPhysX(posX),
    posY: toPhysY(posY),
    selX: toPhysX(cssX1),
    selY: toPhysY(cssY1),
    selW: toPhysW(_cssX2 - cssX1),
    selH: toPhysW(_cssY2 - cssY1),
  }).catch((e) => {
    console.error('Prepare delayed capture failed:', e);
  });
}

function drawSelection() {
  const x1 = Math.min(startX, currentX);
  const y1 = Math.min(startY, currentY);
  const x2 = Math.max(startX, currentX);
  const y2 = Math.max(startY, currentY);
  const w = x2 - x1;
  const h = y2 - y1;

  if (w < 2 || h < 2) return;

  if ((mode === 'freeze') && screenshotImage) {
    ctx.drawImage(dimmedCanvas, 0, 0);
    // Sample the physical-pixel screenshot at scaled coords, draw at CSS coords
    const sx = x1 * scale(), sy = y1 * scale(), sw = w * scale(), sh = h * scale();
    ctx.drawImage(screenshotImage, sx, sy, sw, sh, x1, y1, w, h);
  } else {
    ctx.clearRect(0, 0, canvas.width, canvas.height);
    ctx.fillStyle = 'rgba(0, 0, 0, 0.3)';
    ctx.fillRect(0, 0, canvas.width, canvas.height);
    ctx.clearRect(x1, y1, w, h);
  }

  // Selection border — draw outside the selection in record_region mode
  // so it never appears inside the captured area
  ctx.strokeStyle = '#00aaff';
  ctx.lineWidth = 2;
  if (mode === 'record_region') {
    ctx.strokeRect(x1 - 2, y1 - 2, w + 4, h + 4);
  } else {
    ctx.strokeRect(x1, y1, w, h);
  }

  // Dimension label -- show physical pixel dimensions (actual capture size)
  const physW = Math.round(w * scale());
  const physH = Math.round(h * scale());
  const label = `${physW} x ${physH}`;
  ctx.font = 'bold 13px Consolas, monospace';
  const labelY = y1 > 25 ? y1 - 8 : y2 + 18;
  ctx.fillStyle = 'rgba(0, 0, 0, 0.6)';
  const metrics = ctx.measureText(label);
  ctx.fillRect(x1, labelY - 14, metrics.width + 8, 20);
  ctx.fillStyle = '#00aaff';
  ctx.fillText(label, x1 + 4, labelY);
}

// -- Window capture handlers --

function onMouseMoveWindow() {
  if (windowPollPending) return;
  windowPollPending = true;

  requestAnimationFrame(async () => {
    try {
      const rect = await invoke<WindowRect | null>('get_window_at_cursor');
      if (rect) {
        // Rust returns absolute screen coords in xcap's coordinate space.
        // Subtract overlay origin, then divide by xcapScale for CSS pixels.
        // macOS: xcap uses logical points = CSS pixels (xcapScale = 1).
        // Windows: xcap uses physical pixels (xcapScale = devicePixelRatio).
        highlightRect = {
          x: (rect.left - overlayOriginX) / xcapScale,
          y: (rect.top - overlayOriginY) / xcapScale,
          w: (rect.right - rect.left) / xcapScale,
          h: (rect.bottom - rect.top) / xcapScale,
        };
        // Keep original absolute coords for sending back to Rust
        highlightPhysical = {
          left: rect.left,
          top: rect.top,
          right: rect.right,
          bottom: rect.bottom,
        };
      } else {
        highlightRect = null;
        highlightPhysical = null;
      }
      drawWindowHighlight();
    } finally {
      windowPollPending = false;
    }
  });
}

function drawWindowHighlight() {
  // Clear and redraw the near-invisible background
  ctx.clearRect(0, 0, canvas.width, canvas.height);
  ctx.fillStyle = 'rgba(0, 0, 0, 0.01)';
  ctx.fillRect(0, 0, canvas.width, canvas.height);

  if (highlightRect) {
    const { x, y, w, h } = highlightRect;
    // Blue border around highlighted window
    ctx.strokeStyle = '#00aaff';
    ctx.lineWidth = 3;
    ctx.strokeRect(x, y, w, h);
  }
}

function onMouseDownWindow(shiftKey: boolean, altKey: boolean) {
  if (!highlightPhysical || !highlightRect) return;
  const { x, y, w, h } = highlightRect;
  const phys = { ...highlightPhysical };
  captureWithDelay(x, y, x + w, y + h,
    { mode: 'window', ...phys, forceAnnotate: shiftKey, toggleSave: altKey },
    () => invoke('complete_window_capture', { ...phys, forceAnnotate: shiftKey, toggleSave: altKey })
  );
}

// -- Keyboard --

function onKeyDown(e: KeyboardEvent) {
  if (e.key === 'Escape') {
    // Escape ALWAYS works, even during recording -- never trap the user
    if (isRecordingActive) {
      isRecordingActive = false;
      invoke('stop_recording').catch(() => {});
    }
    cancelled = false; // Reset so cancel() runs even if previously called
    cancel();
  } else if (mode !== 'window' && mode !== 'record_region' && mode !== 'ocr' && (e.key === 'Enter' || e.key === ' ')) {
    e.preventDefault();
    captureFullscreen();
  }
}

function cancel() {
  if (cancelled) return;
  cancelled = true;
  isDragging = false;
  invoke('cancel_capture');
}

function captureFullscreen() {
  if (cancelled) return;
  // Send physical pixel dimensions to Rust
  invoke('complete_region_capture', {
    x1: 0,
    y1: 0,
    x2: Math.round(canvas.width * scale()),
    y2: Math.round(canvas.height * scale()),
  });
}

// Bootstrap: set up overlay, then show window (avoids white flash)
window.addEventListener('DOMContentLoaded', async () => {
  initCanvas();
  try {
    mode = await invoke<string>('get_overlay_mode');
    // Always fetch overlay origin — needed for CSS→physical conversion in all modes
    const origin = await invoke<[number, number]>('get_overlay_origin');
    overlayOriginX = origin[0];
    overlayOriginY = origin[1];

    if (mode === 'select_screen') {
      monitors = await invoke<MonitorInfo[]>('get_monitor_list');
      showSelectScreenOverlay();
    } else if (mode === 'window') {
      showWindowOverlay();
    } else if (mode === 'freeze') {
      const base64Data: string = await invoke('get_pending_screenshot');
      loadScreenshot(base64Data);
    } else if (mode === 'record_region') {
      showRecordRegionOverlay();
    } else if (mode === 'ocr') {
      showOcrOverlay();
    } else {
      showInstantOverlay();
    }
    const win = getCurrentWindow();
    await win.show();
    await win.setFocus();

    // macOS compositor doesn't pick up canvas content drawn before the
    // transparent window is shown. Force a redraw now that the window is
    // visible so the dimmed overlay actually appears immediately.
    requestAnimationFrame(() => {
      if (mode === 'select_screen') {
        showSelectScreenOverlay();
      } else if (mode === 'window') {
        showWindowOverlay();
      } else if (mode === 'record_region') {
        showRecordRegionOverlay();
      } else if (mode === 'ocr') {
        showOcrOverlay();
      } else if (mode !== 'freeze') {
        showInstantOverlay();
      }
      // freeze mode redraws asynchronously in loadScreenshot's img.onload
    });

    // Cancel capture if overlay loses OS focus (e.g. Cmd+Tab, system UI).
    // Small delay avoids false triggers from momentary focus transitions.
    // Skip when recording is active -- the overlay stays as a visual boundary.
    window.addEventListener('blur', () => {
      if (isDragging || isRecordingActive) return;
      setTimeout(() => {
        if (!document.hasFocus() && !isRecordingActive) cancel();
      }, 200);
    });
  } catch (e) {
    console.error('Overlay init failed:', e);
    cancel();
  }
});
