import { normalize, type Rect } from "../lib/selection.ts";
import { appendStrokePoint } from "../lib/stroke.ts";

export { appendStrokePoint };

export type Point = { x: number; y: number };

export type Layer =
  | { id: string; type: "rect"; x: number; y: number; w: number; h: number; color: string; thickness: number; filled: boolean }
  | { id: string; type: "ellipse"; x: number; y: number; w: number; h: number; color: string; thickness: number; filled: boolean }
  | { id: string; type: "arrow"; x1: number; y1: number; x2: number; y2: number; color: string; thickness: number }
  | { id: string; type: "brush"; points: Point[]; color: string; thickness: number }
  | { id: string; type: "mosaic"; points: Point[]; radius: number; block: number }
  | { id: string; type: "highlight"; x: number; y: number; w: number; h: number; ellipse: boolean }
  | { id: string; type: "text"; x: number; y: number; text: string; color: string; size: number }
  | { id: string; type: "number"; x: number; y: number; n: number; color: string }
  | { id: string; type: "watermark"; text: string; color: string; size: number };

export const COLORS = ["#ef4444", "#f59e0b", "#22c55e", "#3b82f6", "#a855f7", "#111827"];
export const MIN_FONT = 14;
export const MAX_FONT = 72;

function textBox(text: string, size: number): { w: number; h: number } {
  const lines = text.split("\n");
  let width = 0;
  for (const line of lines) {
    let lineWidth = 0;
    for (const ch of line) {
      const code = ch.codePointAt(0) ?? 0;
      if (ch === " " || ch === "\t") lineWidth += size * 0.34;
      else if (code > 255) lineWidth += size;
      else lineWidth += size * 0.6;
    }
    width = Math.max(width, lineWidth);
  }
  const height = Math.max(size, (Math.max(lines.length, 1) - 1) * size * 1.25 + size);
  return { w: Math.max(size * 0.6, width + size * 0.25), h: height };
}

let sequence = 0;
export function uid(): string {
  sequence += 1;
  return `l${Date.now().toString(36)}${sequence}`;
}

export function layerBounds(layer: Layer): Rect {
  switch (layer.type) {
    case "rect":
    case "ellipse":
    case "highlight":
      return { x: layer.x, y: layer.y, w: layer.w, h: layer.h };
    case "arrow":
      return normalize(layer.x1, layer.y1, layer.x2, layer.y2);
    case "brush":
    case "mosaic":
      return pointsBounds(layer.points, layer.type === "mosaic" ? layer.radius : layer.thickness);
    case "text": {
      const box = textBox(layer.text, layer.size);
      return { x: layer.x, y: layer.y, w: box.w, h: box.h };
    }
    case "number":
      return { x: layer.x - 16, y: layer.y - 16, w: 32, h: 32 };
    case "watermark":
      return { x: 0, y: 0, w: 0, h: 0 };
  }
}

function pointsBounds(points: Point[], pad: number): Rect {
  if (points.length === 0) return { x: 0, y: 0, w: 0, h: 0 };
  const xs = points.map((point) => point.x);
  const ys = points.map((point) => point.y);
  const x = Math.min(...xs) - pad;
  const y = Math.min(...ys) - pad;
  return { x, y, w: Math.max(...xs) - x + pad, h: Math.max(...ys) - y + pad };
}

export function moveLayer(layer: Layer, dx: number, dy: number): Layer {
  switch (layer.type) {
    case "rect":
    case "ellipse":
    case "highlight":
      return { ...layer, x: layer.x + dx, y: layer.y + dy };
    case "arrow":
      return { ...layer, x1: layer.x1 + dx, y1: layer.y1 + dy, x2: layer.x2 + dx, y2: layer.y2 + dy };
    case "brush":
    case "mosaic":
      return { ...layer, points: layer.points.map((point) => ({ x: point.x + dx, y: point.y + dy })) };
    case "text":
    case "number":
      return { ...layer, x: layer.x + dx, y: layer.y + dy };
    case "watermark":
      return layer;
  }
}

export function hitTest(layers: Layer[], x: number, y: number): string | null {
  for (let index = layers.length - 1; index >= 0; index -= 1) {
    const layer = layers[index];
    if (layer.type === "watermark") continue;
    const box = layerBounds(layer);
    const pad = layer.type === "brush" || layer.type === "arrow" || layer.type === "text" ? 8 : 0;
    if (x >= box.x - pad && y >= box.y - pad && x <= box.x + box.w + pad && y <= box.y + box.h + pad) {
      return layer.id;
    }
  }
  return null;
}

export function resizeTextLayer(layer: Extract<Layer, { type: "text" }>, box: Rect, handle: string): Extract<Layer, { type: "text" }> {
  const old = layerBounds(layer);
  const vertical = handle.includes("n") || handle.includes("s");
  const ratioSource = vertical ? old.h : old.w;
  const nextSpan = vertical ? box.h : box.w;
  const ratio = ratioSource > 0 ? nextSpan / ratioSource : 1;
  const size = Math.min(MAX_FONT, Math.max(MIN_FONT, Math.round(layer.size * ratio)));
  const next = textBox(layer.text, size);
  let x = layer.x;
  let y = layer.y;
  if (handle.includes("n")) y = layer.y + old.h - next.h;
  if (handle.includes("w")) x = layer.x + old.w - next.w;
  return { ...layer, x, y, size };
}

export function resizeBox(box: Rect, handle: string, x: number, y: number): Rect {
  let x0 = box.x;
  let y0 = box.y;
  let x1 = box.x + box.w;
  let y1 = box.y + box.h;
  if (handle.includes("n")) y0 = y;
  if (handle.includes("s")) y1 = y;
  if (handle.includes("w")) x0 = x;
  if (handle.includes("e")) x1 = x;
  const next = normalize(x0, y0, x1, y1);
  return { ...next, w: Math.max(4, next.w), h: Math.max(4, next.h) };
}

export function rotatePoint(x: number, y: number, height: number): Point {
  return { x: height - y, y: x };
}

export function rotateLayer(layer: Layer, _width: number, height: number): Layer {
  const map = (x: number, y: number) => rotatePoint(x, y, height);
  switch (layer.type) {
    case "rect":
    case "ellipse":
    case "highlight": {
      const corners = [map(layer.x, layer.y), map(layer.x + layer.w, layer.y), map(layer.x, layer.y + layer.h), map(layer.x + layer.w, layer.y + layer.h)];
      const xs = corners.map((point) => point.x);
      const ys = corners.map((point) => point.y);
      return { ...layer, x: Math.min(...xs), y: Math.min(...ys), w: Math.max(...xs) - Math.min(...xs), h: Math.max(...ys) - Math.min(...ys) };
    }
    case "arrow": {
      const start = map(layer.x1, layer.y1);
      const end = map(layer.x2, layer.y2);
      return { ...layer, x1: start.x, y1: start.y, x2: end.x, y2: end.y };
    }
    case "brush":
    case "mosaic":
      return { ...layer, points: layer.points.map((point) => map(point.x, point.y)) };
    case "text":
    case "number": {
      const point = map(layer.x, layer.y);
      return { ...layer, x: point.x, y: point.y };
    }
    case "watermark":
      return layer;
  }
}

export function parseLayers(documentText: string): Layer[] {
  try {
    const value = JSON.parse(documentText) as Layer[];
    return Array.isArray(value) ? value : [];
  } catch {
    return [];
  }
}
