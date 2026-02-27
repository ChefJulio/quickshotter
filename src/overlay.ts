import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';

let canvas: HTMLCanvasElement;
let ctx: CanvasRenderingContext2D;
let screenshotImage: HTMLImageElement | null = null;
let dimmedCanvas: HTMLCanvasElement;
let dimmedCtx: CanvasRenderingContext2D;

let isDragging = false;
let startX = 0;
let startY = 0;
let currentX = 0;
let currentY = 0;

function initCanvas() {
  canvas = document.getElementById('overlay-canvas') as HTMLCanvasElement;
  ctx = canvas.getContext('2d')!;
  canvas.width = window.innerWidth;
  canvas.height = window.innerHeight;

  // Pre-create dimmed canvas
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

async function onScreenshotReady(base64Data: string) {
  const img = new Image();
  img.onload = () => {
    screenshotImage = img;

    // Resize canvas to match the screenshot
    canvas.width = img.width;
    canvas.height = img.height;
    dimmedCanvas.width = img.width;
    dimmedCanvas.height = img.height;

    // Draw dimmed version
    dimmedCtx.drawImage(img, 0, 0);
    dimmedCtx.fillStyle = 'rgba(0, 0, 0, 0.45)';
    dimmedCtx.fillRect(0, 0, img.width, img.height);

    // Show dimmed screenshot
    ctx.drawImage(dimmedCanvas, 0, 0);
  };
  img.src = `data:image/png;base64,${base64Data}`;
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
  if (!isDragging || !screenshotImage) return;
  currentX = e.clientX;
  currentY = e.clientY;
  drawSelection();
}

function onMouseUp(e: MouseEvent) {
  if (!isDragging || !screenshotImage) return;
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
  if (!screenshotImage) return;

  const x1 = Math.min(startX, currentX);
  const y1 = Math.min(startY, currentY);
  const x2 = Math.max(startX, currentX);
  const y2 = Math.max(startY, currentY);
  const w = x2 - x1;
  const h = y2 - y1;

  if (w < 2 || h < 2) return;

  // Redraw dimmed background
  ctx.drawImage(dimmedCanvas, 0, 0);

  // Draw bright (original) region inside selection
  ctx.drawImage(screenshotImage, x1, y1, w, h, x1, y1, w, h);

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
  invoke('cancel_capture');
}

function captureFullscreen() {
  if (!screenshotImage) return;
  invoke('complete_region_capture', {
    x1: 0,
    y1: 0,
    x2: screenshotImage.width,
    y2: screenshotImage.height,
  });
}

// Initialize
window.addEventListener('DOMContentLoaded', () => {
  initCanvas();
  listen<string>('screenshot-ready', (event) => {
    onScreenshotReady(event.payload);
  });
});
