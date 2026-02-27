import { invoke } from '@tauri-apps/api/core';
import { getCurrentWindow } from '@tauri-apps/api/window';

let canvas: HTMLCanvasElement;
let ctx: CanvasRenderingContext2D;

// Freeze mode only
let screenshotImage: HTMLImageElement | null = null;
let dimmedCanvas: HTMLCanvasElement;
let dimmedCtx: CanvasRenderingContext2D;

let isDragging = false;
let startX = 0;
let startY = 0;
let currentX = 0;
let currentY = 0;
let mode: string = 'instant';
let cancelled = false;

function initCanvas() {
  canvas = document.getElementById('overlay-canvas') as HTMLCanvasElement;
  ctx = canvas.getContext('2d')!;
  canvas.width = window.innerWidth;
  canvas.height = window.innerHeight;

  // Pre-create dimmed canvas (used in freeze mode)
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
  // Just fill with semi-transparent dark -- no screenshot needed
  ctx.fillStyle = 'rgba(0, 0, 0, 0.3)';
  ctx.fillRect(0, 0, canvas.width, canvas.height);
}

function loadScreenshot(base64Data: string) {
  const img = new Image();
  img.onload = () => {
    screenshotImage = img;

    canvas.width = img.width;
    canvas.height = img.height;
    dimmedCanvas.width = img.width;
    dimmedCanvas.height = img.height;

    // Draw dimmed version
    dimmedCtx.drawImage(img, 0, 0);
    dimmedCtx.fillStyle = 'rgba(0, 0, 0, 0.45)';
    dimmedCtx.fillRect(0, 0, img.width, img.height);

    ctx.drawImage(dimmedCanvas, 0, 0);
  };
  img.src = `data:image/jpeg;base64,${base64Data}`;
}

function onMouseDown(e: MouseEvent) {
  if (e.button !== 0) return;
  isDragging = true;
  startX = e.clientX;
  startY = e.clientY;
  currentX = e.clientX;
  currentY = e.clientY;
}

function onMouseMove(e: MouseEvent) {
  if (!isDragging) return;
  currentX = e.clientX;
  currentY = e.clientY;
  drawSelection();
}

function onMouseUp(e: MouseEvent) {
  if (!isDragging) return;
  isDragging = false;

  const x1 = Math.min(startX, e.clientX);
  const y1 = Math.min(startY, e.clientY);
  const x2 = Math.max(startX, e.clientX);
  const y2 = Math.max(startY, e.clientY);

  if (x2 - x1 < 3 || y2 - y1 < 3) {
    cancel();
    return;
  }

  invoke('complete_region_capture', { x1, y1, x2, y2 });
}

function drawSelection() {
  const x1 = Math.min(startX, currentX);
  const y1 = Math.min(startY, currentY);
  const x2 = Math.max(startX, currentX);
  const y2 = Math.max(startY, currentY);
  const w = x2 - x1;
  const h = y2 - y1;

  if (w < 2 || h < 2) return;

  if (mode === 'freeze' && screenshotImage) {
    // Freeze mode: redraw dimmed bg, show bright region in selection
    ctx.drawImage(dimmedCanvas, 0, 0);
    ctx.drawImage(screenshotImage, x1, y1, w, h, x1, y1, w, h);
  } else {
    // Instant mode: redraw dark overlay, clear selection area to show through
    ctx.clearRect(0, 0, canvas.width, canvas.height);
    ctx.fillStyle = 'rgba(0, 0, 0, 0.3)';
    ctx.fillRect(0, 0, canvas.width, canvas.height);
    ctx.clearRect(x1, y1, w, h);
  }

  // Selection border
  ctx.strokeStyle = '#00aaff';
  ctx.lineWidth = 2;
  ctx.strokeRect(x1, y1, w, h);

  // Dimension label
  const label = `${w} x ${h}`;
  ctx.font = 'bold 13px Consolas, monospace';
  const labelY = y1 > 25 ? y1 - 8 : y2 + 18;
  ctx.fillStyle = 'rgba(0, 0, 0, 0.6)';
  const metrics = ctx.measureText(label);
  ctx.fillRect(x1, labelY - 14, metrics.width + 8, 20);
  ctx.fillStyle = '#00aaff';
  ctx.fillText(label, x1 + 4, labelY);
}

function onKeyDown(e: KeyboardEvent) {
  if (e.key === 'Escape') {
    cancel();
  } else if (e.key === 'Enter' || e.key === ' ') {
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
  invoke('complete_region_capture', {
    x1: 0,
    y1: 0,
    x2: canvas.width,
    y2: canvas.height,
  });
}

// Bootstrap: set up overlay, then show window (avoids white flash)
window.addEventListener('DOMContentLoaded', async () => {
  initCanvas();
  try {
    mode = await invoke<string>('get_overlay_mode');
    if (mode === 'freeze') {
      const base64Data: string = await invoke('get_pending_screenshot');
      loadScreenshot(base64Data);
    } else {
      showInstantOverlay();
    }
    // Show window only after canvas is ready (no white flash)
    const win = getCurrentWindow();
    await win.show();
    await win.setFocus();
  } catch (e) {
    console.error('Overlay init failed:', e);
    cancel();
  }
});
