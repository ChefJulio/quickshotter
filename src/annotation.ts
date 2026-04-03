import './annotation.css';
import { invoke, convertFileSrc } from '@tauri-apps/api/core';
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

interface BlurAnnotation {
  type: 'blur';
  rect: { x: number; y: number; w: number; h: number };
  radius: number;
}

interface StepAnnotation {
  type: 'step';
  position: Point;
  number: number;
  color: string;
  fontSize: number;
}

type Annotation = FreehandAnnotation | ArrowAnnotation | OvalAnnotation | RectAnnotation | TextAnnotation | BlurAnnotation | StepAnnotation;

const MIN_DRAG_SIZE = 3;
const MAX_UNDO = 100;

// -- State --

let canvas: HTMLCanvasElement;
let ctx: CanvasRenderingContext2D;
let sourceImage: HTMLImageElement | HTMLCanvasElement;
let sourceWidth = 0;
let sourceHeight = 0;
let imageRect = { x: 0, y: 0, w: 0, h: 0 };
let scale = 1;

// Zoom/pan state
let zoomLevel = 1;
let panX = 0;
let panY = 0;
let isPanning = false;
let panStart: Point | null = null;
let spaceHeld = false;
const MIN_ZOOM = 0.1;
const MAX_ZOOM = 10;

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
let modifierTools: AnnotationConfig = {
  shift_tool: 'oval', ctrl_tool: 'rect', alt_tool: 'step', default_tool: 'freehand',
  right_default_tool: 'arrow', right_shift_tool: 'blur', right_ctrl_tool: 'text', right_alt_tool: 'grabtext',
};

// Text tool state
let textInputActive = false;
let textInputImagePos: Point | null = null;

// Text dragging state
let draggingTextIdx: number | null = null;
let textDragOffset: Point = { x: 0, y: 0 };
let textDragOriginalPos: Point | null = null; // for undo

// Toolbar dragging
let toolbarDragPos: { x: number; y: number } | null = null;

// Grab-text (OCR) state
let isGrabbingText = false;
let grabTextRect: { x: number; y: number; w: number; h: number } | null = null;
let grabTextDragStart: Point | null = null;

// Crop state
let isCropping = false;
let cropRect: { x: number; y: number; w: number; h: number } | null = null;
let cropDragStart: Point | null = null;
// Edge/corner handle being resized: 'n','s','e','w','nw','ne','sw','se', or null
let cropResizeHandle: string | null = null;

// -- Coordinate transforms --

function cssWidth() { return canvas.width / (window.devicePixelRatio || 1); }
function cssHeight() { return canvas.height / (window.devicePixelRatio || 1); }

function computeLayout() {
  // Use CSS dimensions (not backing-store pixels) since ctx.scale(dpr) is applied
  const sw = cssWidth();
  const sh = cssHeight();
  const iw = sourceWidth;
  const ih = sourceHeight;
  if (iw <= 0 || ih <= 0) {
    imageRect = { x: 0, y: 0, w: sw, h: sh };
    scale = 1;
    return;
  }
  const fitScale = Math.min(sw / iw, sh / ih, 1.0);
  scale = fitScale * zoomLevel;
  const dw = Math.floor(iw * scale);
  const dh = Math.floor(ih * scale);
  imageRect = {
    x: Math.floor((sw - dw) / 2) + panX,
    y: Math.floor((sh - dh) / 2) + panY,
    w: dw,
    h: dh,
  };
}

function resetView() {
  zoomLevel = 1;
  panX = 0;
  panY = 0;
  computeLayout();
  render();
}

function toImageCoords(screenX: number, screenY: number): Point | null {
  const x = (screenX - imageRect.x) / scale;
  const y = (screenY - imageRect.y) / scale;
  if (x < 0 || y < 0 || x >= sourceWidth || y >= sourceHeight) {
    return null;
  }
  return { x, y };
}

function clampToImage(screenX: number, screenY: number): Point {
  const x = Math.max(0, Math.min((screenX - imageRect.x) / scale, sourceWidth - 1));
  const y = Math.max(0, Math.min((screenY - imageRect.y) / scale, sourceHeight - 1));
  return { x, y };
}

// -- Rendering --

function render() {
  const cw = cssWidth();
  const ch = cssHeight();
  ctx.clearRect(0, 0, cw, ch);

  // Dark background
  ctx.fillStyle = 'rgba(0, 0, 0, 0.7)';
  ctx.fillRect(0, 0, cw, ch);

  // Draw image at display size
  ctx.drawImage(sourceImage, imageRect.x, imageRect.y, imageRect.w, imageRect.h);

  // Draw annotations (image-space transform)
  const allAnns = isDrawing && currentAnnotation ? [...undoStack, currentAnnotation] : undoStack;
  for (const ann of allAnns) {
    if (ann.type === 'blur') {
      drawBlurAnnotation(ctx, ann, sourceImage);
    } else {
      ctx.save();
      ctx.translate(imageRect.x, imageRect.y);
      ctx.scale(scale, scale);
      drawAnnotation(ctx, ann);
      ctx.restore();
    }
  }

  // Draw delete button for hovered annotation (in screen-space, after restore)
  if (hoveredAnnotationIdx !== null && hoveredAnnotationIdx < undoStack.length && !isDrawing) {
    const bp = deleteButtonPos(undoStack[hoveredAnnotationIdx]);
    if (bp) {
      const r = 8;
      ctx.save();
      // Circle background
      ctx.beginPath();
      ctx.arc(bp.x, bp.y, r, 0, Math.PI * 2);
      ctx.fillStyle = 'rgba(220, 40, 40, 0.9)';
      ctx.fill();
      // X icon
      ctx.strokeStyle = '#fff';
      ctx.lineWidth = 1.5;
      ctx.lineCap = 'round';
      const d = 3.5;
      ctx.beginPath();
      ctx.moveTo(bp.x - d, bp.y - d);
      ctx.lineTo(bp.x + d, bp.y + d);
      ctx.moveTo(bp.x + d, bp.y - d);
      ctx.lineTo(bp.x - d, bp.y + d);
      ctx.stroke();
      ctx.restore();
    }
  }

  // Grab-text selection overlay
  if (isGrabbingText && grabTextRect && grabTextRect.w > 0 && grabTextRect.h > 0) {
    const r = grabTextRect;
    const sx = imageRect.x + r.x * scale;
    const sy = imageRect.y + r.y * scale;
    const sw = r.w * scale;
    const sh = r.h * scale;
    ctx.strokeStyle = '#00cc66';
    ctx.lineWidth = 2;
    ctx.setLineDash([6, 3]);
    ctx.strokeRect(sx, sy, sw, sh);
    ctx.setLineDash([]);
    // Label
    ctx.font = 'bold 11px Consolas, monospace';
    ctx.fillStyle = 'rgba(0, 204, 102, 0.9)';
    const labelY = sy > 18 ? sy - 5 : sy + sh + 14;
    ctx.fillText('OCR', sx + 2, labelY);
  }

  // Crop overlay (drawn last, on top of everything)
  if (isCropping) {
    drawCropOverlay();
  }
}

function drawAnnotation(c: CanvasRenderingContext2D, ann: Annotation) {
  switch (ann.type) {
    case 'freehand': drawFreehand(c, ann); break;
    case 'arrow': drawArrow(c, ann); break;
    case 'oval': drawOval(c, ann); break;
    case 'rect': drawRect(c, ann); break;
    case 'text': drawText(c, ann); break;
    case 'step': drawStep(c, ann); break;
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
  const metrics = c.measureText(ann.text);
  const ascent = metrics.actualBoundingBoxAscent ?? ann.fontSize * 0.8;
  const descent = metrics.actualBoundingBoxDescent ?? ann.fontSize * 0.2;
  const pad = Math.round(ann.fontSize * 0.15);
  c.fillStyle = 'rgba(0, 0, 0, 0.65)';
  c.fillRect(
    ann.position.x - pad,
    ann.position.y - ascent - pad,
    metrics.width + pad * 2,
    ascent + descent + pad * 2,
  );
  c.fillStyle = ann.color;
  c.fillText(ann.text, ann.position.x, ann.position.y);
}

function drawStep(c: CanvasRenderingContext2D, ann: StepAnnotation) {
  const radius = ann.fontSize * 0.7;
  // Filled circle
  c.beginPath();
  c.arc(ann.position.x, ann.position.y, radius, 0, Math.PI * 2);
  c.fillStyle = ann.color;
  c.fill();
  // Number text (white on colored circle)
  c.font = `bold ${ann.fontSize}px sans-serif`;
  c.textAlign = 'center';
  c.textBaseline = 'middle';
  c.fillStyle = '#ffffff';
  c.fillText(String(ann.number), ann.position.x, ann.position.y);
  c.textAlign = 'start';
  c.textBaseline = 'alphabetic';
}

/** Render a blur annotation. Handles its own coordinate transform since it
 *  needs to clip and re-draw the source image with a CSS filter. */
function drawBlurAnnotation(
  c: CanvasRenderingContext2D,
  ann: BlurAnnotation,
  img: CanvasImageSource,
) {
  const r = ann.rect;
  // Screen-space coordinates for display rendering
  const sx = imageRect.x + r.x * scale;
  const sy = imageRect.y + r.y * scale;
  const sw = r.w * scale;
  const sh = r.h * scale;
  if (sw < 1 || sh < 1) return;

  c.save();
  c.beginPath();
  c.rect(sx, sy, sw, sh);
  c.clip();
  c.filter = `blur(${ann.radius}px)`;
  c.drawImage(img, imageRect.x, imageRect.y, imageRect.w, imageRect.h);
  c.restore();

  // Subtle border so the user can see the blur region
  c.strokeStyle = 'rgba(255, 255, 255, 0.25)';
  c.lineWidth = 1;
  c.setLineDash([4, 4]);
  c.strokeRect(sx, sy, sw, sh);
  c.setLineDash([]);
}

/** Render a blur annotation at full resolution for the composite/save canvas.
 *  No scale transform -- image-space coordinates only. */
function drawBlurAnnotationFullRes(
  c: CanvasRenderingContext2D,
  ann: BlurAnnotation,
  img: CanvasImageSource,
) {
  const r = ann.rect;
  if (r.w < 1 || r.h < 1) return;

  c.save();
  c.beginPath();
  c.rect(r.x, r.y, r.w, r.h);
  c.clip();
  c.filter = `blur(${ann.radius}px)`;
  c.drawImage(img, 0, 0);
  c.restore();
}

// -- Tool determination from modifiers --

function toolFromModifiers(e: MouseEvent): string {
  const isRight = e.button === 2;
  if (e.shiftKey) {
    const t = isRight ? modifierTools.right_shift_tool : modifierTools.shift_tool;
    return t !== 'none' ? t : activeTool;
  }
  if (e.ctrlKey || e.metaKey) {
    const t = isRight ? modifierTools.right_ctrl_tool : modifierTools.ctrl_tool;
    return t !== 'none' ? t : activeTool;
  }
  if (e.altKey) {
    const t = isRight ? modifierTools.right_alt_tool : modifierTools.alt_tool;
    return t !== 'none' ? t : activeTool;
  }
  if (isRight) {
    return modifierTools.right_default_tool;
  }
  return activeTool;
}

// -- Hit testing --

// Hovered annotation index (for delete button + cursor)
let hoveredAnnotationIdx: number | null = null;

function annotationBoundingRect(ann: Annotation): { x: number; y: number; w: number; h: number } | null {
  switch (ann.type) {
    case 'text': return textBoundingRect(ann);
    case 'freehand': {
      if (ann.points.length === 0) return null;
      let minX = Infinity, minY = Infinity, maxX = -Infinity, maxY = -Infinity;
      for (const p of ann.points) {
        minX = Math.min(minX, p.x); minY = Math.min(minY, p.y);
        maxX = Math.max(maxX, p.x); maxY = Math.max(maxY, p.y);
      }
      const pad = ann.width / 2;
      return { x: minX - pad, y: minY - pad, w: maxX - minX + ann.width, h: maxY - minY + ann.width };
    }
    case 'arrow': {
      const x1 = Math.min(ann.start.x, ann.end.x);
      const y1 = Math.min(ann.start.y, ann.end.y);
      const x2 = Math.max(ann.start.x, ann.end.x);
      const y2 = Math.max(ann.start.y, ann.end.y);
      const pad = ann.width * 2;
      return { x: x1 - pad, y: y1 - pad, w: x2 - x1 + pad * 2, h: y2 - y1 + pad * 2 };
    }
    case 'oval':
    case 'rect': {
      const pad = ann.width / 2;
      return { x: ann.rect.x - pad, y: ann.rect.y - pad, w: ann.rect.w + ann.width, h: ann.rect.h + ann.width };
    }
    case 'blur':
      return { ...ann.rect };
    case 'step': {
      const r = ann.fontSize * 0.7;
      return { x: ann.position.x - r, y: ann.position.y - r, w: r * 2, h: r * 2 };
    }
  }
}

function hitTestAnnotation(imagePos: Point): number | null {
  for (let i = undoStack.length - 1; i >= 0; i--) {
    const r = annotationBoundingRect(undoStack[i]);
    if (!r) continue;
    const margin = 6;
    if (
      imagePos.x >= r.x - margin && imagePos.x <= r.x + r.w + margin &&
      imagePos.y >= r.y - margin && imagePos.y <= r.y + r.h + margin
    ) {
      return i;
    }
  }
  return null;
}

function deleteAnnotation(idx: number) {
  undoStack.splice(idx, 1);
  redoStack.length = 0;
  hoveredAnnotationIdx = null;
  updateUndoRedoButtons();
  render();
}

// Returns the screen position of the delete button for the given annotation
function deleteButtonPos(ann: Annotation): { x: number; y: number } | null {
  const r = annotationBoundingRect(ann);
  if (!r) return null;
  // Top-right corner of the bounding box, in screen coords
  return {
    x: imageRect.x + (r.x + r.w) * scale + 4,
    y: imageRect.y + r.y * scale - 4,
  };
}

function textBoundingRect(ann: TextAnnotation): { x: number; y: number; w: number; h: number } {
  // Measure in image-space (no scale transform -- we render at image scale)
  ctx.save();
  ctx.font = `${ann.fontSize}px sans-serif`;
  const metrics = ctx.measureText(ann.text);
  ctx.restore();
  const ascent = metrics.actualBoundingBoxAscent ?? ann.fontSize * 0.8;
  const descent = metrics.actualBoundingBoxDescent ?? ann.fontSize * 0.2;
  return {
    x: ann.position.x,
    y: ann.position.y - ascent,
    w: metrics.width,
    h: ascent + descent,
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
  // Middle-mouse or Space+left-click starts panning
  if (e.button === 1 || (e.button === 0 && spaceHeld)) {
    e.preventDefault();
    isPanning = true;
    panStart = { x: e.clientX - panX, y: e.clientY - panY };
    canvas.style.cursor = 'grabbing';
    return;
  }

  if (e.button !== 0 && e.button !== 2) return;

  // Check if click is on toolbar
  const toolbar = document.getElementById('annotation-toolbar')!;
  const tbRect = toolbar.getBoundingClientRect();
  if (e.clientX >= tbRect.left && e.clientX <= tbRect.right &&
      e.clientY >= tbRect.top && e.clientY <= tbRect.bottom) {
    return; // Let toolbar handle it
  }

  // Also skip crop bar clicks
  const cropBar = document.getElementById('crop-bar')!;
  const cbRect = cropBar.getBoundingClientRect();
  if (cropBar.style.display !== 'none' &&
      e.clientX >= cbRect.left && e.clientX <= cbRect.right &&
      e.clientY >= cbRect.top && e.clientY <= cbRect.bottom) {
    return;
  }

  // Crop mode intercepts all mouse events
  if (isCropping) {
    if (e.button === 0) onCropMouseDown(e);
    return;
  }

  // Check if click is on a delete button
  if (hoveredAnnotationIdx !== null && hoveredAnnotationIdx < undoStack.length) {
    const bp = deleteButtonPos(undoStack[hoveredAnnotationIdx]);
    if (bp && Math.hypot(e.clientX - bp.x, e.clientY - bp.y) <= 10) {
      deleteAnnotation(hoveredAnnotationIdx);
      return;
    }
  }

  const pos = toImageCoords(e.clientX, e.clientY);
  if (!pos) return;

  // Allow dragging existing text annotations regardless of the active tool.
  // This way text placed via a modifier key (e.g. Alt) can still be repositioned
  // without switching to the text tool first.
  const hitIdx = hitTestText(pos);
  if (hitIdx !== null) {
    const ann = undoStack[hitIdx] as TextAnnotation;
    draggingTextIdx = hitIdx;
    textDragOffset = { x: ann.position.x - pos.x, y: ann.position.y - pos.y };
    textDragOriginalPos = { ...ann.position };
    isDrawing = true;
    return;
  }

  const tool = toolFromModifiers(e);

  // Grab-text mode: start drag selection for OCR
  if (tool === 'grabtext') {
    isGrabbingText = true;
    grabTextDragStart = pos;
    grabTextRect = { x: pos.x, y: pos.y, w: 0, h: 0 };
    render();
    return;
  }

  // Text tool: place new (hit test above didn't match an existing annotation)
  if (tool === 'text') {
    placeTextInput(e.clientX, e.clientY, pos);
    return;
  }

  // Step tool: click to place next numbered step
  if (tool === 'step') {
    const nextNum = undoStack.filter(a => a.type === 'step').length + 1;
    undoStack.push({ type: 'step', position: pos, number: nextNum, color: activeColor, fontSize: activeFontSize });
    redoStack.length = 0;
    updateUndoRedoButtons();
    render();
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
  } else if (tool === 'blur') {
    currentAnnotation = { type: 'blur', rect: { x: pos.x, y: pos.y, w: 0, h: 0 }, radius: 12 };
  }

  isDrawing = true;
  hoveredAnnotationIdx = null;
  render();
}

function onMouseMove(e: MouseEvent) {
  // Pan handling
  if (isPanning && panStart) {
    panX = e.clientX - panStart.x;
    panY = e.clientY - panStart.y;
    computeLayout();
    render();
    return;
  }

  if (isCropping) { onCropMouseMove(e); return; }

  // Grab-text drag: update selection rect
  if (isGrabbingText && grabTextDragStart) {
    const pos = clampToImage(e.clientX, e.clientY);
    grabTextRect = {
      x: Math.min(grabTextDragStart.x, pos.x),
      y: Math.min(grabTextDragStart.y, pos.y),
      w: Math.abs(pos.x - grabTextDragStart.x),
      h: Math.abs(pos.y - grabTextDragStart.y),
    };
    render();
    return;
  }

  // Text dragging
  if (draggingTextIdx !== null) {
    const pos = clampToImage(e.clientX, e.clientY);
    const ann = undoStack[draggingTextIdx] as TextAnnotation;
    ann.position = { x: pos.x + textDragOffset.x, y: pos.y + textDragOffset.y };
    render();
    return;
  }

  if (!isDrawing || !currentAnnotation) {
    // Hover detection for cursor + delete button
    const pos = toImageCoords(e.clientX, e.clientY);
    const prevHovered = hoveredAnnotationIdx;
    if (pos) {
      // Check delete button hit first
      if (hoveredAnnotationIdx !== null && hoveredAnnotationIdx < undoStack.length) {
        const bp = deleteButtonPos(undoStack[hoveredAnnotationIdx]);
        if (bp && Math.hypot(e.clientX - bp.x, e.clientY - bp.y) <= 10) {
          canvas.style.cursor = 'pointer';
          return;
        }
      }
      const textHit = hitTestText(pos);
      hoveredAnnotationIdx = hitTestAnnotation(pos);
      if (textHit !== null) {
        canvas.style.cursor = 'move';
      } else if (hoveredAnnotationIdx !== null) {
        canvas.style.cursor = 'default';
      } else {
        canvas.style.cursor = 'crosshair';
      }
    } else {
      hoveredAnnotationIdx = null;
      canvas.style.cursor = 'default';
    }
    if (prevHovered !== hoveredAnnotationIdx) render();
    return;
  }

  const pos = clampToImage(e.clientX, e.clientY);

  if (currentAnnotation.type === 'freehand') {
    currentAnnotation.points.push(pos);
  } else if (currentAnnotation.type === 'arrow') {
    currentAnnotation.end = pos;
  } else if ((currentAnnotation.type === 'oval' || currentAnnotation.type === 'rect' || currentAnnotation.type === 'blur') && dragStart) {
    const x = Math.min(dragStart.x, pos.x);
    const y = Math.min(dragStart.y, pos.y);
    const w = Math.abs(pos.x - dragStart.x);
    const h = Math.abs(pos.y - dragStart.y);
    currentAnnotation.rect = { x, y, w, h };
  }

  render();
}

function onMouseUp(e: MouseEvent) {
  if (isPanning) {
    isPanning = false;
    panStart = null;
    canvas.style.cursor = spaceHeld ? 'grab' : 'crosshair';
    return;
  }

  if (e.button !== 0 && e.button !== 2) return;
  if (isCropping) { onCropMouseUp(); return; }

  // Grab-text: extract region and run OCR
  if (isGrabbingText) {
    isGrabbingText = false;
    grabTextDragStart = null;
    if (grabTextRect && grabTextRect.w >= MIN_DRAG_SIZE && grabTextRect.h >= MIN_DRAG_SIZE) {
      performGrabTextOcr(grabTextRect);
    }
    grabTextRect = null;
    render();
    return;
  }

  // Finish text drag. Position was mutated in-place during onMouseMove.
  // The flat undo stack model doesn't support undoing in-place edits, so
  // we just clear redo to prevent inconsistency from stale redo entries.
  if (draggingTextIdx !== null) {
    if (textDragOriginalPos) {
      const ann = undoStack[draggingTextIdx] as TextAnnotation;
      const dx = ann.position.x - textDragOriginalPos.x;
      const dy = ann.position.y - textDragOriginalPos.y;
      if (Math.abs(dx) > 1 || Math.abs(dy) > 1) {
        redoStack.length = 0;
        updateUndoRedoButtons();
      }
    }
    draggingTextIdx = null;
    textDragOriginalPos = null;
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
  } else if (currentAnnotation.type === 'oval' || currentAnnotation.type === 'rect' || currentAnnotation.type === 'blur') {
    if (currentAnnotation.rect.w < MIN_DRAG_SIZE && currentAnnotation.rect.h < MIN_DRAG_SIZE) discard = true;
  }

  if (!discard) {
    undoStack.push(currentAnnotation);
    if (undoStack.length > MAX_UNDO) undoStack.shift();
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
    if (undoStack.length > MAX_UNDO) undoStack.shift();
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
  if (undoStack.length > MAX_UNDO) undoStack.shift();
  updateUndoRedoButtons();
  render();
}

function updateUndoRedoButtons() {
  (document.getElementById('btn-undo') as HTMLButtonElement).disabled = undoStack.length === 0;
  (document.getElementById('btn-redo') as HTMLButtonElement).disabled = redoStack.length === 0;
}

// -- Grab-text (OCR) tool --

async function performGrabTextOcr(rect: { x: number; y: number; w: number; h: number }) {
  // Extract region from source image at full resolution
  const cx = Math.max(0, Math.round(rect.x));
  const cy = Math.max(0, Math.round(rect.y));
  const cw = Math.min(Math.round(rect.w), sourceWidth - cx);
  const ch = Math.min(Math.round(rect.h), sourceHeight - cy);
  if (cw < 1 || ch < 1) return;

  const offscreen = document.createElement('canvas');
  offscreen.width = cw;
  offscreen.height = ch;
  const offCtx = offscreen.getContext('2d')!;
  offCtx.drawImage(sourceImage, cx, cy, cw, ch, 0, 0, cw, ch);

  // Convert to base64 PNG (strip the data URL prefix)
  const dataUrl = offscreen.toDataURL('image/png');
  const base64 = dataUrl.replace(/^data:image\/png;base64,/, '');

  try {
    await invoke<string>('ocr_image', { imageBase64: base64 });
  } catch (err) {
    console.error('Grab text OCR failed:', err);
  }
}

// -- Crop tool --

function enterCropMode() {
  isCropping = true;
  cropRect = { x: 0, y: 0, w: sourceWidth, h: sourceHeight };
  cropResizeHandle = null;
  document.getElementById('crop-bar')!.style.display = 'flex';
  canvas.style.cursor = 'default';
  render();
}

function exitCropMode() {
  isCropping = false;
  cropRect = null;
  cropDragStart = null;
  cropResizeHandle = null;
  document.getElementById('crop-bar')!.style.display = 'none';
  canvas.style.cursor = 'default';
  render();
}

function applyCrop() {
  if (!cropRect || cropRect.w < 2 || cropRect.h < 2) { exitCropMode(); return; }

  // Bake current annotations + image into an offscreen canvas, then crop
  const offscreen = document.createElement('canvas');
  offscreen.width = sourceWidth;
  offscreen.height = sourceHeight;
  const offCtx = offscreen.getContext('2d')!;
  offCtx.drawImage(sourceImage, 0, 0);
  for (const ann of undoStack) {
    drawAnnotation(offCtx, ann);
  }

  // Crop region (clamped to image bounds)
  const cx = Math.max(0, Math.round(cropRect.x));
  const cy = Math.max(0, Math.round(cropRect.y));
  const cw = Math.min(Math.round(cropRect.w), offscreen.width - cx);
  const ch = Math.min(Math.round(cropRect.h), offscreen.height - cy);
  if (cw < 1 || ch < 1) { exitCropMode(); return; }

  // Extract cropped region
  const cropped = document.createElement('canvas');
  cropped.width = cw;
  cropped.height = ch;
  const croppedCtx = cropped.getContext('2d')!;
  croppedCtx.drawImage(offscreen, cx, cy, cw, ch, 0, 0, cw, ch);

  // Replace source image with cropped canvas directly — no encoding
  sourceImage = cropped;
  sourceWidth = cw;
  sourceHeight = ch;
  undoStack.length = 0;
  redoStack.length = 0;
  updateUndoRedoButtons();
  computeLayout();
  exitCropMode();
}

function drawCropOverlay() {
  if (!cropRect) return;

  // Convert image-space crop rect to screen-space
  const sx = imageRect.x + cropRect.x * scale;
  const sy = imageRect.y + cropRect.y * scale;
  const sw = cropRect.w * scale;
  const sh = cropRect.h * scale;

  // Dim outside crop area
  ctx.fillStyle = 'rgba(0, 0, 0, 0.5)';
  // Top
  ctx.fillRect(imageRect.x, imageRect.y, imageRect.w, sy - imageRect.y);
  // Bottom
  ctx.fillRect(imageRect.x, sy + sh, imageRect.w, (imageRect.y + imageRect.h) - (sy + sh));
  // Left
  ctx.fillRect(imageRect.x, sy, sx - imageRect.x, sh);
  // Right
  ctx.fillRect(sx + sw, sy, (imageRect.x + imageRect.w) - (sx + sw), sh);

  // Crop border
  ctx.strokeStyle = '#00aaff';
  ctx.lineWidth = 2;
  ctx.setLineDash([6, 3]);
  ctx.strokeRect(sx, sy, sw, sh);
  ctx.setLineDash([]);

  // Corner/edge handles
  const hs = 6; // handle size
  ctx.fillStyle = '#00aaff';
  const handles = getCropHandlePositions(sx, sy, sw, sh, hs);
  for (const h of Object.values(handles)) {
    ctx.fillRect(h.x - hs / 2, h.y - hs / 2, hs, hs);
  }

  // Dimensions label
  const dimText = `${Math.round(cropRect.w)} x ${Math.round(cropRect.h)}`;
  ctx.font = '12px Consolas, monospace';
  ctx.fillStyle = '#00aaff';
  ctx.textAlign = 'center';
  const labelY = sy > 20 ? sy - 6 : sy + sh + 16;
  ctx.fillText(dimText, sx + sw / 2, labelY);
  ctx.textAlign = 'start';
}

function getCropHandlePositions(sx: number, sy: number, sw: number, sh: number, _hs: number) {
  return {
    nw: { x: sx, y: sy },
    n:  { x: sx + sw / 2, y: sy },
    ne: { x: sx + sw, y: sy },
    w:  { x: sx, y: sy + sh / 2 },
    e:  { x: sx + sw, y: sy + sh / 2 },
    sw: { x: sx, y: sy + sh },
    s:  { x: sx + sw / 2, y: sy + sh },
    se: { x: sx + sw, y: sy + sh },
  };
}

function hitTestCropHandle(screenX: number, screenY: number): string | null {
  if (!cropRect) return null;
  const sx = imageRect.x + cropRect.x * scale;
  const sy = imageRect.y + cropRect.y * scale;
  const sw = cropRect.w * scale;
  const sh = cropRect.h * scale;
  const tolerance = 8;
  const handles = getCropHandlePositions(sx, sy, sw, sh, 6);
  for (const [name, pos] of Object.entries(handles)) {
    if (Math.abs(screenX - pos.x) <= tolerance && Math.abs(screenY - pos.y) <= tolerance) {
      return name;
    }
  }
  return null;
}

function getCropCursor(handle: string | null): string {
  if (!handle) return cropRect ? 'move' : 'crosshair';
  const cursors: Record<string, string> = {
    nw: 'nwse-resize', se: 'nwse-resize',
    ne: 'nesw-resize', sw: 'nesw-resize',
    n: 'ns-resize', s: 'ns-resize',
    e: 'ew-resize', w: 'ew-resize',
  };
  return cursors[handle] || 'default';
}

function onCropMouseDown(e: MouseEvent) {
  const handle = hitTestCropHandle(e.clientX, e.clientY);
  if (handle) {
    cropResizeHandle = handle;
    cropDragStart = { x: e.clientX, y: e.clientY };
    return;
  }
  // If clicking inside existing crop rect, start move
  if (cropRect) {
    const sx = imageRect.x + cropRect.x * scale;
    const sy = imageRect.y + cropRect.y * scale;
    const sw = cropRect.w * scale;
    const sh = cropRect.h * scale;
    if (e.clientX >= sx && e.clientX <= sx + sw && e.clientY >= sy && e.clientY <= sy + sh) {
      cropResizeHandle = 'move';
      cropDragStart = { x: e.clientX, y: e.clientY };
      return;
    }
  }
  // Start new crop selection
  const pos = toImageCoords(e.clientX, e.clientY);
  if (!pos) return;
  cropDragStart = { x: e.clientX, y: e.clientY };
  cropRect = { x: pos.x, y: pos.y, w: 0, h: 0 };
  cropResizeHandle = 'new';
}

function onCropMouseMove(e: MouseEvent) {
  if (!cropDragStart) {
    // Update cursor for handle hover
    const handle = hitTestCropHandle(e.clientX, e.clientY);
    if (handle) {
      canvas.style.cursor = getCropCursor(handle);
    } else if (cropRect) {
      const sx = imageRect.x + cropRect.x * scale;
      const sy = imageRect.y + cropRect.y * scale;
      const sw = cropRect.w * scale;
      const sh = cropRect.h * scale;
      if (e.clientX >= sx && e.clientX <= sx + sw && e.clientY >= sy && e.clientY <= sy + sh) {
        canvas.style.cursor = 'move';
      } else {
        canvas.style.cursor = 'crosshair';
      }
    }
    return;
  }

  const iw = sourceWidth;
  const ih = sourceHeight;

  if (cropResizeHandle === 'new') {
    // Drawing new crop rect
    const start = toImageCoords(cropDragStart.x, cropDragStart.y);
    const end = clampToImage(e.clientX, e.clientY);
    if (start) {
      cropRect = {
        x: Math.min(start.x, end.x),
        y: Math.min(start.y, end.y),
        w: Math.abs(end.x - start.x),
        h: Math.abs(end.y - start.y),
      };
    }
  } else if (cropResizeHandle === 'move' && cropRect) {
    const dx = (e.clientX - cropDragStart.x) / scale;
    const dy = (e.clientY - cropDragStart.y) / scale;
    cropRect.x = Math.max(0, Math.min(cropRect.x + dx, iw - cropRect.w));
    cropRect.y = Math.max(0, Math.min(cropRect.y + dy, ih - cropRect.h));
    cropDragStart = { x: e.clientX, y: e.clientY };
  } else if (cropRect && cropResizeHandle) {
    // Resize via handle
    const dx = (e.clientX - cropDragStart.x) / scale;
    const dy = (e.clientY - cropDragStart.y) / scale;
    const r = { ...cropRect };

    if (cropResizeHandle.includes('w')) { r.x += dx; r.w -= dx; }
    if (cropResizeHandle.includes('e')) { r.w += dx; }
    if (cropResizeHandle.includes('n')) { r.y += dy; r.h -= dy; }
    if (cropResizeHandle.includes('s')) { r.h += dy; }

    // Clamp to image bounds and enforce minimum size
    if (r.w < 10) { r.w = 10; if (cropResizeHandle.includes('w')) r.x = cropRect.x + cropRect.w - 10; }
    if (r.h < 10) { r.h = 10; if (cropResizeHandle.includes('n')) r.y = cropRect.y + cropRect.h - 10; }
    r.x = Math.max(0, r.x);
    r.y = Math.max(0, r.y);
    if (r.x + r.w > iw) r.w = iw - r.x;
    if (r.y + r.h > ih) r.h = ih - r.y;

    cropRect = r;
    cropDragStart = { x: e.clientX, y: e.clientY };
  }

  render();
}

function onCropMouseUp() {
  cropDragStart = null;
  cropResizeHandle = null;
}

// -- Save / Cancel --

let saving = false;

async function compositeAndSave() {
  if (saving) { console.log('[save] already saving, skipping'); return; }
  saving = true;
  console.log('[save] compositeAndSave started');
  try {
    // Create offscreen canvas at original image dimensions
    const offscreen = document.createElement('canvas');
    offscreen.width = sourceWidth;
    offscreen.height = sourceHeight;
    console.log(`[save] offscreen canvas: ${offscreen.width}x${offscreen.height}`);
    const offCtx = offscreen.getContext('2d')!;

    // Draw original image at full resolution
    offCtx.drawImage(sourceImage, 0, 0);

    // Draw all annotations in image-space (no scale needed)
    for (const ann of undoStack) {
      if (ann.type === 'blur') {
        drawBlurAnnotationFullRes(offCtx, ann, sourceImage);
      } else {
        drawAnnotation(offCtx, ann);
      }
    }
    console.log(`[save] annotations drawn (${undoStack.length} items)`);

    // Export as PNG base64
    const dataUrl = offscreen.toDataURL('image/png');
    const base64 = dataUrl.split(',')[1];
    console.log(`[save] base64 encoded, length=${base64?.length ?? 'null'}`);

    await invoke('save_annotated_capture', { imageBase64: base64 });
    console.log('[save] invoke completed');
  } catch (e) {
    console.error('[save] failed:', e);
    saving = false;
  }
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
    if (isCropping) { exitCropMode(); return; }
    cancelAnnotation();
  } else if (e.key === 'Enter') {
    if (isCropping) { applyCrop(); return; }
    compositeAndSave();
  } else if (e.key === 'z' && (e.ctrlKey || e.metaKey) && !e.shiftKey) {
    e.preventDefault();
    undo();
  } else if (e.key === 'z' && (e.ctrlKey || e.metaKey) && e.shiftKey) {
    e.preventDefault();
    redo();
  } else if (e.key === '0' && (e.ctrlKey || e.metaKey)) {
    e.preventDefault();
    resetView();
  } else if (e.key === ' ') {
    e.preventDefault();
    spaceHeld = true;
    canvas.style.cursor = 'grab';
  }
}

function onKeyUp(e: KeyboardEvent) {
  if (e.key === ' ') {
    spaceHeld = false;
    if (!isPanning) {
      canvas.style.cursor = 'crosshair';
    }
  }
}

function onWheel(e: WheelEvent) {
  e.preventDefault();

  if (e.ctrlKey) {
    // Ctrl+scroll = zoom centered on cursor
    const zoomFactor = e.deltaY < 0 ? 1.1 : 1 / 1.1;
    const newZoom = Math.max(MIN_ZOOM, Math.min(MAX_ZOOM, zoomLevel * zoomFactor));
    if (newZoom === zoomLevel) return;

    const sw = cssWidth();
    const sh = cssHeight();
    const iw = sourceWidth;
    const ih = sourceHeight;
    const fitScale = Math.min(sw / iw, sh / ih, 1.0);

    const oldScale = fitScale * zoomLevel;
    const oldCenterX = (sw - iw * oldScale) / 2 + panX;
    const oldCenterY = (sh - ih * oldScale) / 2 + panY;

    const imgX = (e.clientX - oldCenterX) / oldScale;
    const imgY = (e.clientY - oldCenterY) / oldScale;

    const newScale = fitScale * newZoom;
    const newCenterX = (sw - iw * newScale) / 2;
    const newCenterY = (sh - ih * newScale) / 2;

    panX = e.clientX - imgX * newScale - newCenterX;
    panY = e.clientY - imgY * newScale - newCenterY;
    zoomLevel = newZoom;
  } else {
    // Regular scroll = pan (vertical scroll pans Y, shift+scroll pans X)
    const delta = e.shiftKey ? { x: -e.deltaY, y: 0 } : { x: -e.deltaX, y: -e.deltaY };
    panX += delta.x;
    panY += delta.y;
  }

  computeLayout();
  render();
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
    crop: document.getElementById('tool-crop')!,
    blur: document.getElementById('tool-blur')!,
    step: document.getElementById('tool-step')!,
    grabtext: document.getElementById('tool-grabtext')!,
  };

  for (const [tool, btn] of Object.entries(toolBtns)) {
    btn.addEventListener('click', () => {
      if (tool === 'crop') {
        if (isCropping) { exitCropMode(); } else { enterCropMode(); }
        updateToolbarHighlights(toolBtns);
        return;
      }
      if (isCropping) exitCropMode();
      activeTool = tool;
      updateToolbarHighlights(toolBtns);
    });
  }

  document.getElementById('crop-apply')!.addEventListener('click', applyCrop);
  document.getElementById('crop-cancel')!.addEventListener('click', exitCropMode);

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
    if (tool === 'crop') {
      btn.classList.toggle('active', isCropping);
    } else {
      btn.classList.toggle('active', !isCropping && tool === activeTool);
    }
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

  // Scale canvas backing store for Retina/HiDPI displays.
  // CSS size stays at window dimensions; backing store is multiplied by DPR
  // so drawings render at native resolution instead of blurry 1x.
  const dpr = window.devicePixelRatio || 1;
  canvas.width = window.innerWidth * dpr;
  canvas.height = window.innerHeight * dpr;
  canvas.style.width = `${window.innerWidth}px`;
  canvas.style.height = `${window.innerHeight}px`;
  ctx.scale(dpr, dpr);

  canvas.addEventListener('mousedown', onMouseDown);
  canvas.addEventListener('mousemove', onMouseMove);
  canvas.addEventListener('mouseup', onMouseUp);
  canvas.addEventListener('contextmenu', (e) => {
    e.preventDefault();
  });
  canvas.addEventListener('wheel', onWheel, { passive: false });
  document.addEventListener('keydown', onKeyDown);
  document.addEventListener('keyup', onKeyUp);

  // Re-layout on window resize
  window.addEventListener('resize', () => {
    const dpr = window.devicePixelRatio || 1;
    canvas.width = window.innerWidth * dpr;
    canvas.height = window.innerHeight * dpr;
    canvas.style.width = `${window.innerWidth}px`;
    canvas.style.height = `${window.innerHeight}px`;
    ctx.setTransform(1, 0, 0, 1, 0, 0);
    ctx.scale(dpr, dpr);
    if (sourceImage) {
      computeLayout();
      render();
    }
  });

  try {
    // Load modifier-to-tool config (includes default_tool)
    modifierTools = await invoke<AnnotationConfig>('get_annotation_config');
    if (modifierTools.default_tool && modifierTools.default_tool !== 'none') {
      activeTool = modifierTools.default_tool;
    }

    // Init toolbar AFTER default_tool is set so highlights are correct
    initToolbar();

    // Position toolbar below system UI (e.g. macOS menu bar) to prevent clipping.
    // screen.availTop gives the first y-coordinate not occupied by system chrome.
    const toolbar = document.getElementById('annotation-toolbar')!;
    const safeTop = Math.max(8, ((window.screen as any).availTop || 0) + 8);
    toolbar.style.top = `${safeTop}px`;

    // Load raw RGBA bytes directly — no encoding/decoding overhead.
    // Rust writes raw pixels to disk (~5ms), JS creates ImageData from them.
    const [filePath, imgW, imgH] = await invoke<[string, number, number]>('get_pending_annotation');
    console.log(`[annotation] loading raw RGBA: ${filePath} (${imgW}x${imgH})`);
    const assetUrl = convertFileSrc(filePath);
    console.log(`[annotation] asset URL: ${assetUrl}`);
    const response = await fetch(assetUrl);
    if (!response.ok) {
      console.error(`[annotation] fetch failed: ${response.status} ${response.statusText}`);
      cancelAnnotation();
      return;
    }
    const buffer = await response.arrayBuffer();
    console.log(`[annotation] loaded ${buffer.byteLength} bytes, expected ${imgW * imgH * 4}`);

    // Create canvas with raw pixel data — zero encoding
    const rawCanvas = document.createElement('canvas');
    rawCanvas.width = imgW;
    rawCanvas.height = imgH;
    const rawCtx = rawCanvas.getContext('2d')!;
    const imageData = new ImageData(new Uint8ClampedArray(buffer), imgW, imgH);
    rawCtx.putImageData(imageData, 0, 0);

    sourceImage = rawCanvas;
    sourceWidth = imgW;
    sourceHeight = imgH;
    computeLayout();
    render();

    const win = getCurrentWindow();
    await win.show();
    await win.setFocus();
  } catch (e) {
    console.error('Annotation editor init failed:', e);
    cancelAnnotation();
  }
});
