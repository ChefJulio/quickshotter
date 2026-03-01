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

// DPI scale factor: CSS pixels * scale = physical pixels (what Rust uses).
// In freeze/window mode, derived from the actual screenshot dimensions
// for correct mixed-DPI multi-monitor mapping. Falls back to devicePixelRatio.
let captureScale: number | null = null;
function scale(): number {
  return captureScale ?? (window.devicePixelRatio || 1);
}

// Window capture state
let highlightRect: { x: number; y: number; w: number; h: number } | null = null;
// Physical-pixel coords from Rust, sent back directly for capture
let highlightPhysical: { left: number; top: number; right: number; bottom: number } | null = null;
let windowPollPending = false;

function initCanvas() {
  canvas = document.getElementById('overlay-canvas') as HTMLCanvasElement;
  ctx = canvas.getContext('2d')!;
  // Canvas buffer at CSS pixel resolution; all drawing uses CSS coords
  canvas.width = window.innerWidth;
  canvas.height = window.innerHeight;

  // Pre-create dimmed canvas (used in freeze/window mode)
  dimmedCanvas = document.createElement('canvas');
  dimmedCtx = dimmedCanvas.getContext('2d')!;

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
  if (e.button !== 0) return;

  if (mode === 'window') {
    onMouseDownWindow();
    return;
  }

  isDragging = true;
  startX = e.clientX;
  startY = e.clientY;
  currentX = e.clientX;
  currentY = e.clientY;
}

function onMouseMove(e: MouseEvent) {
  if (mode === 'window') {
    onMouseMoveWindow();
    return;
  }

  if (!isDragging) return;
  currentX = e.clientX;
  currentY = e.clientY;
  drawSelection();
}

function onMouseUp(e: MouseEvent) {
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
  invoke('complete_region_capture', { x1, y1, x2, y2 }).catch((e) => {
    console.error('Region capture failed:', e);
    cancel();
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

  // Selection border
  ctx.strokeStyle = '#00aaff';
  ctx.lineWidth = 2;
  ctx.strokeRect(x1, y1, w, h);

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
      if (rect && screenshotImage) {
        // Rust returns physical pixel coords; convert to CSS for canvas drawing
        highlightRect = {
          x: rect.left / scale(),
          y: rect.top / scale(),
          w: (rect.right - rect.left) / scale(),
          h: (rect.bottom - rect.top) / scale(),
        };
        // Keep original physical coords for sending back to Rust
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
  if (!screenshotImage) return;

  // Redraw dimmed background
  ctx.drawImage(dimmedCanvas, 0, 0);

  if (highlightRect) {
    const { x, y, w, h } = highlightRect;
    // Sample the physical-pixel screenshot at scaled coords, draw at CSS coords
    const sx = x * scale(), sy = y * scale(), sw = w * scale(), sh = h * scale();
    ctx.drawImage(screenshotImage, sx, sy, sw, sh, x, y, w, h);

    // Blue border around highlighted window
    ctx.strokeStyle = '#00aaff';
    ctx.lineWidth = 3;
    ctx.strokeRect(x, y, w, h);
  }
}

function onMouseDownWindow() {
  if (!highlightPhysical) return;
  // Send physical pixel coords directly to Rust
  invoke('complete_window_capture', highlightPhysical).catch((e) => {
    console.error('Window capture failed:', e);
    cancel();
  });
}

// -- Keyboard --

function onKeyDown(e: KeyboardEvent) {
  if (e.key === 'Escape') {
    cancel();
  } else if (mode !== 'window' && (e.key === 'Enter' || e.key === ' ')) {
    e.preventDefault();
    captureFullscreen();
  }
}

function cancel() {
  if (cancelled) return;
  cancelled = true;
  invoke('cancel_capture');
}

function captureFullscreen() {
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
    if (mode === 'freeze' || mode === 'window') {
      const base64Data: string = await invoke('get_pending_screenshot');
      loadScreenshot(base64Data);
    } else {
      showInstantOverlay();
    }
    const win = getCurrentWindow();
    await win.show();
    await win.setFocus();

    // Cancel capture if overlay loses OS focus (e.g. Cmd+Tab, system UI).
    // Small delay avoids false triggers from momentary focus transitions.
    window.addEventListener('blur', () => {
      if (isDragging) return; // don't interrupt active selection
      setTimeout(() => {
        if (!document.hasFocus()) cancel();
      }, 200);
    });
  } catch (e) {
    console.error('Overlay init failed:', e);
    cancel();
  }
});
