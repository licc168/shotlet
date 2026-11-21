import type { Layer, Point } from "./model";
import type { Rect } from "../lib/selection";

export function renderScene(
  ctx: CanvasRenderingContext2D,
  image: CanvasImageSource,
  selection: Rect,
  layers: Layer[],
  viewScale: number,
  paintBase = true,
) {
  const width = Math.max(1, Math.round(selection.w * viewScale));
  const height = Math.max(1, Math.round(selection.h * viewScale));
  ctx.canvas.width = width;
  ctx.canvas.height = height;
  ctx.setTransform(viewScale, 0, 0, viewScale, 0, 0);
  ctx.clearRect(0, 0, selection.w, selection.h);
  // The live preview leaves the photo on the <img> under this canvas.
  // Drawing it again resamples the selection and looks softer than that photo.
  if (paintBase) paintPhoto(ctx, image, selection);
  paintMarks(ctx, layers, selection);
  const mosaics = layers.filter((layer) => layer.type === "mosaic");
  if (mosaics.length === 0) return;
  const snapshot = paintBase
    ? ctx.getImageData(0, 0, width, height)
    : sampleScene(image, selection, layers, viewScale, width, height);
  // The preview canvas sits on the photo, so only mosaic blocks stay opaque.
  // Clear once. Clearing inside each stroke erases the mosaic before it.
  if (!paintBase) {
    const pixels = snapshot.data;
    for (let index = 3; index < pixels.length; index += 4) pixels[index] = 0;
  }
  for (const layer of mosaics) pixelate(snapshot, layer, selection, viewScale, !paintBase);
  ctx.setTransform(1, 0, 0, 1, 0, 0);
  if (paintBase) {
    ctx.putImageData(snapshot, 0, 0);
    return;
  }
  blitMasked(ctx, snapshot);
}

function paintPhoto(ctx: CanvasRenderingContext2D, image: CanvasImageSource, selection: Rect) {
  ctx.imageSmoothingEnabled = true;
  ctx.imageSmoothingQuality = "high";
  ctx.drawImage(image, selection.x, selection.y, selection.w, selection.h, 0, 0, selection.w, selection.h);
}

function paintMarks(ctx: CanvasRenderingContext2D, layers: Layer[], selection: Rect) {
  ctx.save();
  ctx.translate(-selection.x, -selection.y);
  for (const layer of layers) {
    if (layer.type !== "mosaic") drawLayer(ctx, layer, selection);
  }
  ctx.restore();
}

function sampleScene(
  image: CanvasImageSource,
  selection: Rect,
  layers: Layer[],
  viewScale: number,
  width: number,
  height: number,
): ImageData {
  const canvas = document.createElement("canvas");
  canvas.width = width;
  canvas.height = height;
  const sample = canvas.getContext("2d", { willReadFrequently: true });
  if (!sample) return new ImageData(width, height);
  sample.setTransform(viewScale, 0, 0, viewScale, 0, 0);
  paintPhoto(sample, image, selection);
  paintMarks(sample, layers, selection);
  return sample.getImageData(0, 0, width, height);
}

function blitMasked(ctx: CanvasRenderingContext2D, snapshot: ImageData) {
  const canvas = document.createElement("canvas");
  canvas.width = snapshot.width;
  canvas.height = snapshot.height;
  const sample = canvas.getContext("2d");
  if (!sample) return;
  sample.putImageData(snapshot, 0, 0);
  ctx.imageSmoothingEnabled = false;
  ctx.drawImage(canvas, 0, 0);
}

function drawLayer(ctx: CanvasRenderingContext2D, layer: Exclude<Layer, { type: "mosaic" }>, selection: Rect) {
  ctx.save();
  ctx.lineJoin = "round";
  ctx.lineCap = "round";
  switch (layer.type) {
    case "rect":
    case "ellipse":
      traceShape(ctx, layer);
      ctx.strokeStyle = layer.color;
      ctx.fillStyle = layer.color;
      ctx.lineWidth = layer.thickness;
      if (layer.filled) ctx.fill();
      else ctx.stroke();
      break;
    case "arrow":
      drawArrow(ctx, layer.x1, layer.y1, layer.x2, layer.y2, layer.color, layer.thickness);
      break;
    case "brush":
      strokePoints(ctx, layer.points, layer.color, layer.thickness);
      break;
    case "highlight":
      traceShape(ctx, { ...layer, type: layer.ellipse ? "ellipse" : "rect" });
      ctx.fillStyle = highlightFill(layer.color);
      ctx.fill();
      break;
    case "text":
      ctx.fillStyle = layer.color;
      ctx.font = `${layer.size}px "Segoe UI", sans-serif`;
      ctx.textBaseline = "top";
      layer.text.split("\n").forEach((line, index) => {
        ctx.fillText(line, layer.x, layer.y + index * layer.size * 1.25);
      });
      break;
    case "number":
      ctx.beginPath();
      ctx.fillStyle = layer.color;
      ctx.arc(layer.x, layer.y, 15, 0, Math.PI * 2);
      ctx.fill();
      ctx.fillStyle = "#fff";
      ctx.font = "bold 14px Segoe UI";
      ctx.textAlign = "center";
      ctx.textBaseline = "middle";
      ctx.fillText(String(layer.n), layer.x, layer.y + 1);
      break;
    case "watermark":
      ctx.globalAlpha = 0.18;
      ctx.fillStyle = layer.color;
      ctx.font = `${layer.size}px Segoe UI`;
      ctx.translate(selection.x, selection.y);
      for (let y = 24; y < selection.h; y += layer.size * 3) {
        for (let x = 12; x < selection.w; x += layer.size * Math.max(4, layer.text.length)) {
          ctx.fillText(layer.text, x, y);
        }
      }
      break;
  }
  ctx.restore();
}

/** CSS size that puts one captured pixel on one device pixel. */
export function screenCssSize(pixelW: number, pixelH: number, dpr: number): { width: number; height: number } {
  const ratio = Number.isFinite(dpr) && dpr > 0 ? dpr : 1;
  return { width: pixelW / ratio, height: pixelH / ratio };
}

/** Canvas pixels per captured pixel. Stay at 1 when the screen can show the photo; never enlarge it. */
export function previewScale(selectionW: number, selectionH: number, cssW: number, cssH: number, dpr: number): number {
  if (!(selectionW > 0) || !(selectionH > 0)) return 1;
  const ratio = Number.isFinite(dpr) && dpr > 0 ? dpr : 1;
  if (!(cssW > 0) || !(cssH > 0)) return 1;
  const fit = Math.min((cssW * ratio) / selectionW, (cssH * ratio) / selectionH);
  if (!Number.isFinite(fit) || fit <= 0) return 1;
  return Math.min(1, fit);
}

export function highlightFill(color: string | undefined): string {
  const hex = /^#([0-9a-f]{6})$/i.exec(color ?? "");
  const value = hex?.[1] ?? "facc15";
  const red = Number.parseInt(value.slice(0, 2), 16);
  const green = Number.parseInt(value.slice(2, 4), 16);
  const blue = Number.parseInt(value.slice(4, 6), 16);
  return `rgba(${red}, ${green}, ${blue}, 0.38)`;
}

function traceShape(ctx: CanvasRenderingContext2D, layer: { type: string; x: number; y: number; w: number; h: number }) {
  ctx.beginPath();
  if (layer.type === "ellipse") {
    ctx.ellipse(layer.x + layer.w / 2, layer.y + layer.h / 2, Math.abs(layer.w) / 2, Math.abs(layer.h) / 2, 0, 0, Math.PI * 2);
  } else {
    ctx.rect(layer.x, layer.y, layer.w, layer.h);
  }
}

function strokePoints(ctx: CanvasRenderingContext2D, points: Point[], color: string, thickness: number) {
  if (points.length === 0) return;
  ctx.strokeStyle = color;
  ctx.fillStyle = color;
  ctx.lineWidth = thickness;
  if (points.length === 1) {
    ctx.beginPath();
    ctx.arc(points[0].x, points[0].y, Math.max(0.5, thickness / 2), 0, Math.PI * 2);
    ctx.fill();
    return;
  }
  ctx.beginPath();
  ctx.moveTo(points[0].x, points[0].y);
  if (points.length === 2) {
    ctx.lineTo(points[1].x, points[1].y);
    ctx.stroke();
    return;
  }
  ctx.lineTo((points[0].x + points[1].x) / 2, (points[0].y + points[1].y) / 2);
  for (let index = 1; index < points.length - 1; index += 1) {
    const next = points[index + 1];
    ctx.quadraticCurveTo(points[index].x, points[index].y, (points[index].x + next.x) / 2, (points[index].y + next.y) / 2);
  }
  const last = points[points.length - 1];
  ctx.lineTo(last.x, last.y);
  ctx.stroke();
}

function drawArrow(ctx: CanvasRenderingContext2D, x1: number, y1: number, x2: number, y2: number, color: string, thickness: number) {
  const angle = Math.atan2(y2 - y1, x2 - x1);
  const head = Math.max(12, thickness * 4);
  ctx.strokeStyle = color;
  ctx.fillStyle = color;
  ctx.lineWidth = thickness;
  ctx.beginPath();
  ctx.moveTo(x1, y1);
  ctx.lineTo(x2, y2);
  ctx.stroke();
  ctx.beginPath();
  ctx.moveTo(x2, y2);
  ctx.lineTo(x2 - head * Math.cos(angle - 0.4), y2 - head * Math.sin(angle - 0.4));
  ctx.lineTo(x2 - head * Math.cos(angle + 0.4), y2 - head * Math.sin(angle + 0.4));
  ctx.closePath();
  ctx.fill();
}

function pixelate(snapshot: ImageData, layer: Extract<Layer, { type: "mosaic" }>, selection: Rect, viewScale: number, isolate = false) {
  const block = Math.max(2, Math.round(layer.block * viewScale));
  const pixels = snapshot.data;
  const width = snapshot.width;
  const height = snapshot.height;
  const minX = Math.max(0, Math.floor((Math.min(...layer.points.map((point) => point.x)) - layer.radius - selection.x) * viewScale));
  const minY = Math.max(0, Math.floor((Math.min(...layer.points.map((point) => point.y)) - layer.radius - selection.y) * viewScale));
  const maxX = Math.min(width, Math.ceil((Math.max(...layer.points.map((point) => point.x)) + layer.radius - selection.x) * viewScale));
  const maxY = Math.min(height, Math.ceil((Math.max(...layer.points.map((point) => point.y)) + layer.radius - selection.y) * viewScale));
  for (let y = minY; y < maxY; y += block) {
    for (let x = minX; x < maxX; x += block) {
      const imageX = selection.x + (x + block / 2) / viewScale;
      const imageY = selection.y + (y + block / 2) / viewScale;
      if (!nearStroke(imageX, imageY, layer.points, layer.radius)) continue;
      const bw = Math.min(block, width - x);
      const bh = Math.min(block, height - y);
      let r = 0;
      let g = 0;
      let b = 0;
      let count = 0;
      for (let yy = 0; yy < bh; yy += 1) {
        for (let xx = 0; xx < bw; xx += 1) {
          const offset = ((y + yy) * width + (x + xx)) * 4;
          r += pixels[offset];
          g += pixels[offset + 1];
          b += pixels[offset + 2];
          count += 1;
        }
      }
      if (count === 0) continue;
      r = Math.round(r / count);
      g = Math.round(g / count);
      b = Math.round(b / count);
      for (let yy = 0; yy < bh; yy += 1) {
        for (let xx = 0; xx < bw; xx += 1) {
          const offset = ((y + yy) * width + (x + xx)) * 4;
          pixels[offset] = r;
          pixels[offset + 1] = g;
          pixels[offset + 2] = b;
          if (isolate) pixels[offset + 3] = 255;
        }
      }
    }
  }
}

function nearStroke(x: number, y: number, points: Point[], radius: number): boolean {
  const radiusSq = radius * radius;
  for (let index = 0; index < points.length; index += 1) {
    const start = points[index];
    const end = points[Math.min(points.length - 1, index + 1)];
    if (distanceToSegment(x, y, start, end) <= radiusSq) return true;
  }
  return false;
}

function distanceToSegment(x: number, y: number, start: Point, end: Point): number {
  const dx = end.x - start.x;
  const dy = end.y - start.y;
  const length = dx * dx + dy * dy;
  const t = length === 0 ? 0 : Math.max(0, Math.min(1, ((x - start.x) * dx + (y - start.y) * dy) / length));
  const px = start.x + t * dx - x;
  const py = start.y + t * dy - y;
  return px * px + py * py;
}

/** Painted selection as RGBA. Scale 1 matches one captured pixel. */
export function renderRgba(image: CanvasImageSource, selection: Rect, layers: Layer[]): { pixels: Uint8ClampedArray<ArrayBuffer>; width: number; height: number } {
  const canvas = document.createElement("canvas");
  const ctx = canvas.getContext("2d", { willReadFrequently: true });
  if (!ctx) throw new Error("canvas");
  renderScene(ctx, image, selection, layers, 1);
  const frame = ctx.getImageData(0, 0, canvas.width, canvas.height);
  return { pixels: frame.data, width: canvas.width, height: canvas.height };
}

export type RenderedPng = { base64: string; bytes: Uint8Array<ArrayBuffer> };

export async function renderPng(image: CanvasImageSource, selection: Rect, layers: Layer[], annotate: boolean): Promise<RenderedPng> {
  const canvas = document.createElement("canvas");
  const ctx = canvas.getContext("2d");
  if (!ctx) throw new Error("canvas");
  renderScene(ctx, image, selection, annotate ? layers : [], 1);
  const blob = await new Promise<Blob | null>((resolve) => canvas.toBlob(resolve, "image/png"));
  if (!blob) throw new Error("png");
  const bytes = new Uint8Array(await blob.arrayBuffer());
  let binary = "";
  const chunk = 0x4000;
  for (let index = 0; index < bytes.length; index += chunk) {
    binary += String.fromCharCode(...bytes.subarray(index, index + chunk));
  }
  return { base64: btoa(binary), bytes };
}
