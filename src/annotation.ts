import { invoke } from '@tauri-apps/api/core';
import { getCurrentWindow } from '@tauri-apps/api/window';
import type { AnnotationConfig } from './types';

// -- Data model --

interface Point { x: number; y: number; }

interface FreehandAnnotation {
  type: 'freehand';
  points: Point[];
  color: string;
  width: number;
}

interface ArrowAnnotation {
  type: 'arrow';
  start: Point;
  end: Point;
  color: string;
  width: number;
  style: string; // "open" | "standard" | "double"
}

interface OvalAnnotation {
  type: 'oval';
  rect: { x: number; y: number; w: number; h: number };
  color: string;
  width: number;
}

interface RectAnnotation {
  type: 'rect';
  rect: { x: number; y: number; w: number; h: number };
  color: string;
  width: number;
}

interface TextAnnotation {
  type: 'text';
  position: Point;
  text: string;
  color: string;
  fontSize: number;
}

type Annotation = FreehandAnnotation | ArrowAnnotation | OvalAnnotation | RectAnnotation | TextAnnotation;

const MIN_DRAG_SIZE = 3;

// -- State --

let canvas: HTMLCanvasElement;
let ctx: CanvasRenderingContext2D;
let sourceImage: HTMLImageElement;
let imageRect = { x: 0, y: 0, w: 0, h: 0 };
let scale = 1;

let undoStack: Annotation[] = [];
let redoStack: Annotation[] = [];
let currentAnnotation: Annotation | null = null;
let isDrawing = false;
let dragStart: Point | null = null;

// Toolbar state
let activeTool = 'freehand';
let activeColor = '#ff0000';
let activeWidth = 3;
let activeFontSize = 16;
let arrowStyle = 'standard';

// Modifier-to-tool mapping
let modifierTools: AnnotationConfig = { shift_tool: 'arrow', ctrl_tool: 'oval', alt_tool: 'text', default_tool: 'freehand' };

// Text tool state
let textInputActive = false;
let textInputImagePos: Point | null = null;

// Text dragging state
let draggingTextIdx: number | null = null;
let textDragOffset: Point = { x: 0, y: 0 };

// Toolbar dragging
let toolbarDragPos: { x: number; y: number } | null = null;

// -- Coordinate transforms --

function computeLayout() {
  const sw = canvas.width;
  const sh = canvas.height;
  const iw = sourceImage.naturalWidth;
  const ih = sourceImage.naturalHeight;
  if (iw <= 0 || ih <= 0) {
    imageRect = { x: 0, y: 0, w: sw, h: sh };
    scale = 1;
    return;
  }
  scale = Math.min(sw / iw, sh / ih, 1.0);
  const dw = Math.floor(iw * scale);
  const dh = Math.floor(ih * scale);
  imageRect = {
    x: Math.floor((sw - dw) / 2),
    y: Math.floor((sh - dh) / 2),
    w: dw,
    h: dh,
  };
}

function toImageCoords(screenX: number, screenY: number): Point | null {
  const x = (screenX - imageRect.x) / scale;
  const y = (screenY - imageRect.y) / scale;
  if (x < 0 || y < 0 || x >= sourceImage.naturalWidth || y >= sourceImage.naturalHeight) {
    return null;
  }
  return { x, y };
}

function clampToImage(screenX: number, screenY: number): Point {
  const x = Math.max(0, Math.min((screenX - imageRect.x) / scale, sourceImage.naturalWidth - 1));
  const y = Math.max(0, Math.min((screenY - imageRect.y) / scale, sourceImage.naturalHeight - 1));
  return { x, y };
}

// -- Rendering --

function render() {
  ctx.clearRect(0, 0, canvas.width, canvas.height);

  // Dark background
  ctx.fillStyle = 'rgba(0, 0, 0, 0.7)';
  ctx.fillRect(0, 0, canvas.width, canvas.height);

  // Draw image at display size
  ctx.drawImage(sourceImage, imageRect.x, imageRect.y, imageRect.w, imageRect.h);

  // Transform for annotations (image-space)
  ctx.save();
  ctx.translate(imageRect.x, imageRect.y);
  ctx.scale(scale, scale);

  for (const ann of undoStack) {
    drawAnnotation(ctx, ann);
  }
  if (isDrawing && currentAnnotation) {
    drawAnnotation(ctx, currentAnnotation);
  }

  ctx.restore();
}

function drawAnnotation(c: CanvasRenderingContext2D, ann: Annotation) {
  switch (ann.type) {
    case 'freehand': drawFreehand(c, ann); break;
    case 'arrow': drawArrow(c, ann); break;
    case 'oval': drawOval(c, ann); break;
    case 'rect': drawRect(c, ann); break;
    case 'text': drawText(c, ann); break;
  }
}

function drawFreehand(c: CanvasRenderingContext2D, ann: FreehandAnnotation) {
  if (ann.points.length < 2) return;
  c.beginPath();
  c.moveTo(ann.points[0].x, ann.points[0].y);
  for (let i = 1; i < ann.points.length; i++) {
    c.lineTo(ann.points[i].x, ann.points[i].y);
  }
  c.strokeStyle = ann.color;
  c.lineWidth = ann.width;
  c.lineCap = 'round';
  c.lineJoin = 'round';
  c.stroke();
}

function drawArrow(c: CanvasRenderingContext2D, ann: ArrowAnnotation) {
  const dx = ann.end.x - ann.start.x;
  const dy = ann.end.y - ann.start.y;
  const length = Math.hypot(dx, dy);
  if (length < 1) return;

  const angle = Math.atan2(dy, dx);
  let headSize = ann.width * 4;
  headSize = Math.min(headSize, length * 0.45);
  headSize = Math.max(headSize, 6);

  if (ann.style === 'open') {
    drawArrowHollow(c, ann, angle, ann.width, headSize);
  } else {
    drawArrowFilled(c, ann, angle, ann.width, headSize);
  }
}

function drawArrowFilled(c: CanvasRenderingContext2D, ann: ArrowAnnotation, angle: number, shaftWidth: number, headSize: number) {
  const spread = 25 * Math.PI / 180;

  // Shaft line -- pulled back from tip to avoid overlap
  let shaftStartX = ann.start.x;
  let shaftStartY = ann.start.y;
  const shaftEndX = ann.end.x - Math.cos(angle) * headSize * 0.7;
  const shaftEndY = ann.end.y - Math.sin(angle) * headSize * 0.7;

  if (ann.style === 'double') {
    shaftStartX = ann.start.x + Math.cos(angle) * headSize * 0.7;
    shaftStartY = ann.start.y + Math.sin(angle) * headSize * 0.7;
  }

  c.beginPath();
  c.moveTo(shaftStartX, shaftStartY);
  c.lineTo(shaftEndX, shaftEndY);
  c.strokeStyle = ann.color;
  c.lineWidth = shaftWidth;
  c.lineCap = 'butt';
  c.lineJoin = 'miter';
  c.stroke();

  // Head at end
  drawFilledHead(c, ann.end, angle, headSize, spread, ann.color);

  // Head at start (double style)
  if (ann.style === 'double') {
    drawFilledHead(c, ann.start, angle + Math.PI, headSize, spread, ann.color);
  }
}

function drawFilledHead(c: CanvasRenderingContext2D, tip: Point, angle: number, size: number, spread: number, color: string) {
  const p1x = tip.x - size * Math.cos(angle - spread);
  const p1y = tip.y - size * Math.sin(angle - spread);
  const p2x = tip.x - size * Math.cos(angle + spread);
  const p2y = tip.y - size * Math.sin(angle + spread);

  c.beginPath();
  c.moveTo(tip.x, tip.y);
  c.lineTo(p1x, p1y);
  c.lineTo(p2x, p2y);
  c.closePath();
  c.fillStyle = color;
  c.fill();
}

function drawArrowHollow(c: CanvasRenderingContext2D, ann: ArrowAnnotation, angle: number, shaftWidth: number, headSize: number) {
  const spread = 30 * Math.PI / 180;
  const perpX = -Math.sin(angle);
  const perpY = Math.cos(angle);
  const halfShaft = shaftWidth * 0.5;
  const headWing = headSize * Math.sin(spread);

  // Shaft corners at start
  const sTop = { x: ann.start.x + perpX * halfShaft, y: ann.start.y + perpY * halfShaft };
  const sBot = { x: ann.start.x - perpX * halfShaft, y: ann.start.y - perpY * halfShaft };

  // Where head begins
  const hx = ann.end.x - Math.cos(angle) * headSize;
  const hy = ann.end.y - Math.sin(angle) * headSize;
  const hTop = { x: hx + perpX * halfShaft, y: hy + perpY * halfShaft };
  const hBot = { x: hx - perpX * halfShaft, y: hy - perpY * halfShaft };

  // Wing tips
  const wingTop = { x: hx + perpX * headWing, y: hy + perpY * headWing };
  const wingBot = { x: hx - perpX * headWing, y: hy - perpY * headWing };

  c.beginPath();
  c.moveTo(sTop.x, sTop.y);
  c.lineTo(hTop.x, hTop.y);
  c.lineTo(wingTop.x, wingTop.y);
  c.lineTo(ann.end.x, ann.end.y);
  c.lineTo(wingBot.x, wingBot.y);
  c.lineTo(hBot.x, hBot.y);
  c.lineTo(sBot.x, sBot.y);
  c.closePath();

  c.strokeStyle = ann.color;
  c.lineWidth = Math.max(1.5, shaftWidth * 0.4);
  c.lineCap = 'round';
  c.lineJoin = 'miter';
  c.stroke();
}

function drawOval(c: CanvasRenderingContext2D, ann: OvalAnnotation) {
  const cx = ann.rect.x + ann.rect.w / 2;
  const cy = ann.rect.y + ann.rect.h / 2;
  const rx = Math.abs(ann.rect.w) / 2;
  const ry = Math.abs(ann.rect.h) / 2;
  if (rx < 1 || ry < 1) return;
  c.beginPath();
  c.ellipse(cx, cy, rx, ry, 0, 0, Math.PI * 2);
  c.strokeStyle = ann.color;
  c.lineWidth = ann.width;
  c.stroke();
}

function drawRect(c: CanvasRenderingContext2D, ann: RectAnnotation) {
  c.strokeStyle = ann.color;
  c.lineWidth = ann.width;
  c.strokeRect(ann.rect.x, ann.rect.y, ann.rect.w, ann.rect.h);
}

function drawText(c: CanvasRenderingContext2D, ann: TextAnnotation) {
  c.font = `${ann.fontSize}px sans-serif`;
  c.fillStyle = ann.color;
  c.fillText(ann.text, ann.position.x, ann.position.y);
}

// -- Tool determination from modifiers --

function toolFromModifiers(e: MouseEvent): string {
  if (e.shiftKey) {
    const t = modifierTools.shift_tool;
    return t !== 'none' ? t : activeTool;
  }
  if (e.ctrlKey || e.metaKey) {
    const t = modifierTools.ctrl_tool;
    return t !== 'none' ? t : activeTool;
  }
  if (e.altKey) {
    const t = modifierTools.alt_tool;
    return t !== 'none' ? t : activeTool;
  }
  return activeTool;
}

// -- Text hit testing --

function textBoundingRect(ann: TextAnnotation): { x: number; y: number; w: number; h: number } {
  // Measure in image-space (no scale transform -- we render at image scale)
  ctx.save();
  ctx.font = `${ann.fontSize}px sans-serif`;
  const metrics = ctx.measureText(ann.text);
  ctx.restore();
  return {
    x: ann.position.x,
    y: ann.position.y - ann.fontSize * 0.8,
    w: metrics.width,
    h: ann.fontSize * 1.2,
  };
}

function hitTestText(imagePos: Point): number | null {
  for (let i = undoStack.length - 1; i >= 0; i--) {
    const ann = undoStack[i];
    if (ann.type === 'text') {
      const r = textBoundingRect(ann);
      const margin = 4;
      if (
        imagePos.x >= r.x - margin && imagePos.x <= r.x + r.w + margin &&
        imagePos.y >= r.y - margin && imagePos.y <= r.y + r.h + margin
      ) {
        return i;
      }
    }
  }
  return null;
}

// -- Mouse handlers --

function onMouseDown(e: MouseEvent) {
  if (e.button !== 0) return;

  // Check if click is on toolbar
  const toolbar = document.getElementById('annotation-toolbar')!;
  const tbRect = toolbar.getBoundingClientRect();
  if (e.clientX >= tbRect.left && e.clientX <= tbRect.right &&
      e.clientY >= tbRect.top && e.clientY <= tbRect.bottom) {
    return; // Let toolbar handle it
  }

  const pos = toImageCoords(e.clientX, e.clientY);
  if (!pos) return;

  const tool = toolFromModifiers(e);

  // Text tool: drag existing or place new
  if (tool === 'text') {
    const hitIdx = hitTestText(pos);
    if (hitIdx !== null) {
      const ann = undoStack[hitIdx] as TextAnnotation;
      draggingTextIdx = hitIdx;
      textDragOffset = { x: ann.position.x - pos.x, y: ann.position.y - pos.y };
      isDrawing = true;
      return;
    }
    placeTextInput(e.clientX, e.clientY, pos);
    return;
  }

  const color = activeColor;
  const width = activeWidth;
  dragStart = pos;

  if (tool === 'freehand') {
    currentAnnotation = { type: 'freehand', points: [pos], color, width };
  } else if (tool === 'arrow') {
    currentAnnotation = { type: 'arrow', start: pos, end: { ...pos }, color, width, style: arrowStyle };
  } else if (tool === 'oval') {
    currentAnnotation = { type: 'oval', rect: { x: pos.x, y: pos.y, w: 0, h: 0 }, color, width };
  } else if (tool === 'rect') {
    currentAnnotation = { type: 'rect', rect: { x: pos.x, y: pos.y, w: 0, h: 0 }, color, width };
  }

  isDrawing = true;
  render();
}

function onMouseMove(e: MouseEvent) {
  // Text dragging
  if (draggingTextIdx !== null) {
    const pos = clampToImage(e.clientX, e.clientY);
    const ann = undoStack[draggingTextIdx] as TextAnnotation;
    ann.position = { x: pos.x + textDragOffset.x, y: pos.y + textDragOffset.y };
    render();
    return;
  }

  if (!isDrawing || !currentAnnotation) return;

  const pos = clampToImage(e.clientX, e.clientY);

  if (currentAnnotation.type === 'freehand') {
    currentAnnotation.points.push(pos);
  } else if (currentAnnotation.type === 'arrow') {
    currentAnnotation.end = pos;
  } else if ((currentAnnotation.type === 'oval' || currentAnnotation.type === 'rect') && dragStart) {
    const x = Math.min(dragStart.x, pos.x);
    const y = Math.min(dragStart.y, pos.y);
    const w = Math.abs(pos.x - dragStart.x);
    const h = Math.abs(pos.y - dragStart.y);
    currentAnnotation.rect = { x, y, w, h };
  }

  render();
}

function onMouseUp(e: MouseEvent) {
  if (e.button !== 0) return;

  // Finish text drag
  if (draggingTextIdx !== null) {
    draggingTextIdx = null;
    isDrawing = false;
    render();
    return;
  }

  if (!isDrawing) return;
  isDrawing = false;
  if (!currentAnnotation) return;

  // Check minimum size
  let discard = false;
  if (currentAnnotation.type === 'freehand') {
    if (currentAnnotation.points.length < 2) discard = true;
  } else if (currentAnnotation.type === 'arrow') {
    const dx = currentAnnotation.end.x - currentAnnotation.start.x;
    const dy = currentAnnotation.end.y - currentAnnotation.start.y;
    if (Math.hypot(dx, dy) < MIN_DRAG_SIZE) discard = true;
  } else if (currentAnnotation.type === 'oval' || currentAnnotation.type === 'rect') {
    if (currentAnnotation.rect.w < MIN_DRAG_SIZE && currentAnnotation.rect.h < MIN_DRAG_SIZE) discard = true;
  }

  if (!discard) {
    undoStack.push(currentAnnotation);
    redoStack.length = 0;
    updateUndoRedoButtons();
  }

  currentAnnotation = null;
  render();
}

// -- Text tool --

function placeTextInput(screenX: number, screenY: number, imagePos: Point) {
  if (textInputActive) cancelTextInput();

  textInputActive = true;
  textInputImagePos = imagePos;

  const container = document.getElementById('text-input-container')!;
  const input = document.getElementById('text-input') as HTMLInputElement;

  // Style to match current color/size
  input.style.color = activeColor;
  input.style.borderColor = activeColor;
  input.style.fontSize = `${activeFontSize}px`;
  input.value = '';

  container.style.display = 'block';
  container.style.left = `${screenX}px`;
  container.style.top = `${screenY}px`;
  // Delay focus to ensure the input is visible and the canvas mousedown has finished
  requestAnimationFrame(() => input.focus());
}

function confirmTextInput() {
  const input = document.getElementById('text-input') as HTMLInputElement;
  const text = input.value.trim();
  if (text && textInputImagePos) {
    const ann: TextAnnotation = {
      type: 'text',
      position: textInputImagePos,
      text,
      color: activeColor,
      fontSize: activeFontSize,
    };
    undoStack.push(ann);
    redoStack.length = 0;
    updateUndoRedoButtons();
    render();
  }
  removeTextInput();
}

function cancelTextInput() {
  removeTextInput();
}

function removeTextInput() {
  textInputActive = false;
  textInputImagePos = null;
  const container = document.getElementById('text-input-container')!;
  container.style.display = 'none';
}

// -- Undo / Redo --

function undo() {
  if (isDrawing || undoStack.length === 0) return;
  const ann = undoStack.pop()!;
  redoStack.push(ann);
  updateUndoRedoButtons();
  render();
}

function redo() {
  if (isDrawing || redoStack.length === 0) return;
  const ann = redoStack.pop()!;
  undoStack.push(ann);
  updateUndoRedoButtons();
  render();
}

function updateUndoRedoButtons() {
  (document.getElementById('btn-undo') as HTMLButtonElement).disabled = undoStack.length === 0;
  (document.getElementById('btn-redo') as HTMLButtonElement).disabled = redoStack.length === 0;
}

// -- Save / Cancel --

function compositeAndSave() {
  // Create offscreen canvas at original image dimensions
  const offscreen = document.createElement('canvas');
  offscreen.width = sourceImage.naturalWidth;
  offscreen.height = sourceImage.naturalHeight;
  const offCtx = offscreen.getContext('2d')!;

  // Draw original image at full resolution
  offCtx.drawImage(sourceImage, 0, 0);

  // Draw all annotations in image-space (no scale needed)
  for (const ann of undoStack) {
    drawAnnotation(offCtx, ann);
  }

  // Export as PNG base64
  const dataUrl = offscreen.toDataURL('image/png');
  const base64 = dataUrl.split(',')[1];

  invoke('save_annotated_capture', { imageBase64: base64 });
}

function cancelAnnotation() {
  invoke('cancel_annotation');
}

// -- Keyboard --

function onKeyDown(e: KeyboardEvent) {
  // If text input is active, route keys there
  if (textInputActive) {
    if (e.key === 'Escape') {
      e.preventDefault();
      cancelTextInput();
    } else if (e.key === 'Enter') {
      e.preventDefault();
      confirmTextInput();
    }
    return;
  }

  if (e.key === 'Escape') {
    cancelAnnotation();
  } else if (e.key === 'Enter') {
    compositeAndSave();
  } else if (e.key === 'z' && (e.ctrlKey || e.metaKey) && !e.shiftKey) {
    e.preventDefault();
    undo();
  } else if (e.key === 'z' && (e.ctrlKey || e.metaKey) && e.shiftKey) {
    e.preventDefault();
    redo();
  }
}

// -- Toolbar interaction --

let toolBtns: Record<string, HTMLElement> = {};

function initToolbar() {
  toolBtns = {
    freehand: document.getElementById('tool-freehand')!,
    arrow: document.getElementById('tool-arrow')!,
    oval: document.getElementById('tool-oval')!,
    rect: document.getElementById('tool-rect')!,
    text: document.getElementById('tool-text')!,
  };

  for (const [tool, btn] of Object.entries(toolBtns)) {
    btn.addEventListener('click', () => {
      activeTool = tool;
      updateToolbarHighlights(toolBtns);
    });
  }

  document.getElementById('arrow-style')!.addEventListener('change', (e) => {
    arrowStyle = (e.target as HTMLSelectElement).value;
  });

  document.getElementById('color-picker')!.addEventListener('input', (e) => {
    activeColor = (e.target as HTMLInputElement).value;
  });

  document.getElementById('stroke-width')!.addEventListener('change', (e) => {
    activeWidth = parseInt((e.target as HTMLInputElement).value, 10) || 3;
  });

  document.getElementById('font-size')!.addEventListener('change', (e) => {
    activeFontSize = parseInt((e.target as HTMLInputElement).value, 10) || 16;
  });

  document.getElementById('btn-undo')!.addEventListener('click', undo);
  document.getElementById('btn-redo')!.addEventListener('click', redo);
  document.getElementById('btn-save')!.addEventListener('click', compositeAndSave);
  document.getElementById('btn-cancel')!.addEventListener('click', cancelAnnotation);

  // Toolbar dragging
  const toolbar = document.getElementById('annotation-toolbar')!;
  const handle = toolbar.querySelector('.drag-handle')!;
  handle.addEventListener('mousedown', (e: Event) => {
    const me = e as MouseEvent;
    toolbarDragPos = { x: me.clientX - toolbar.offsetLeft, y: me.clientY - toolbar.offsetTop };
    me.preventDefault();
  });

  // Text input key handlers -- stop ALL keys from bubbling to the document handler
  const textInput = document.getElementById('text-input') as HTMLInputElement;
  textInput.addEventListener('keydown', (e: KeyboardEvent) => {
    e.stopPropagation(); // prevent document onKeyDown from interfering
    if (e.key === 'Enter') {
      e.preventDefault();
      confirmTextInput();
    } else if (e.key === 'Escape') {
      e.preventDefault();
      cancelTextInput();
    }
  });
  // Prevent clicks on the text input from reaching the canvas
  const textContainer = document.getElementById('text-input-container')!;
  textContainer.addEventListener('mousedown', (e) => e.stopPropagation());

  updateToolbarHighlights(toolBtns);
}

function updateToolbarHighlights(toolBtns: Record<string, HTMLElement>) {
  for (const [tool, btn] of Object.entries(toolBtns)) {
    btn.classList.toggle('active', tool === activeTool);
  }
}

// -- Global mouse handlers for toolbar drag --

document.addEventListener('mousemove', (e: MouseEvent) => {
  if (toolbarDragPos) {
    const toolbar = document.getElementById('annotation-toolbar')!;
    let x = e.clientX - toolbarDragPos.x;
    let y = e.clientY - toolbarDragPos.y;
    x = Math.max(0, Math.min(x, window.innerWidth - toolbar.offsetWidth));
    y = Math.max(0, Math.min(y, window.innerHeight - toolbar.offsetHeight));
    toolbar.style.left = `${x}px`;
    toolbar.style.top = `${y}px`;
    toolbar.style.transform = 'none';
  }
});

document.addEventListener('mouseup', () => {
  toolbarDragPos = null;
});

// -- Init --

window.addEventListener('DOMContentLoaded', async () => {
  canvas = document.getElementById('annotation-canvas') as HTMLCanvasElement;
  ctx = canvas.getContext('2d')!;
  canvas.width = window.innerWidth;
  canvas.height = window.innerHeight;

  initToolbar();

  canvas.addEventListener('mousedown', onMouseDown);
  canvas.addEventListener('mousemove', onMouseMove);
  canvas.addEventListener('mouseup', onMouseUp);
  canvas.addEventListener('contextmenu', (e) => {
    e.preventDefault();
    cancelAnnotation();
  });
  document.addEventListener('keydown', onKeyDown);

  try {
    // Load modifier-to-tool config (includes default_tool)
    modifierTools = await invoke<AnnotationConfig>('get_annotation_config');
    if (modifierTools.default_tool && modifierTools.default_tool !== 'none') {
      activeTool = modifierTools.default_tool;
      updateToolbarHighlights(toolBtns);
    }

    // Load the captured image
    const base64Data: string = await invoke('get_pending_annotation');
    const img = new Image();
    img.onload = async () => {
      sourceImage = img;
      computeLayout();
      render();

      const win = getCurrentWindow();
      await win.show();
      await win.setFocus();
    };
    img.src = `data:image/png;base64,${base64Data}`;
  } catch (e) {
    console.error('Annotation editor init failed:', e);
    cancelAnnotation();
  }
});
