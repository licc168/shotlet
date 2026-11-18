export type Rect = { x: number; y: number; w: number; h: number };

export function normalize(x0: number, y0: number, x1: number, y1: number): Rect {
  const x = Math.min(x0, x1);
  const y = Math.min(y0, y1);
  return { x, y, w: Math.abs(x1 - x0), h: Math.abs(y1 - y0) };
}

export function square(x0: number, y0: number, x1: number, y1: number): Rect {
  const dx = x1 - x0;
  const dy = y1 - y0;
  const side = Math.max(Math.abs(dx), Math.abs(dy));
  const sx = Math.sign(dx) || 1;
  const sy = Math.sign(dy) || 1;
  return normalize(x0, y0, x0 + sx * side, y0 + sy * side);
}

export function clampRect(rect: Rect, bounds: Rect): Rect {
  let { x, y, w, h } = rect;
  w = Math.min(w, bounds.w);
  h = Math.min(h, bounds.h);
  x = Math.min(Math.max(x, bounds.x), bounds.x + bounds.w - w);
  y = Math.min(Math.max(y, bounds.y), bounds.y + bounds.h - h);
  return { x, y, w, h };
}

export function contains(rect: Rect, x: number, y: number, pad = 0): boolean {
  return x >= rect.x - pad && y >= rect.y - pad && x <= rect.x + rect.w + pad && y <= rect.y + rect.h + pad;
}

// WebView2 emits a pointermove at (0, 0), or far outside the webview, on mouseup.
export function isDragGlitch(point: { x: number; y: number }, last: { x: number; y: number }, bounds: { w: number; h: number }): boolean {
  const jumpedToOrigin = point.x <= 1 && point.y <= 1 && (last.x > 32 || last.y > 32);
  const outside = point.x < -2 || point.y < -2 || point.x > bounds.w + 2 || point.y > bounds.h + 2;
  return jumpedToOrigin || outside;
}

export function intersects(a: Rect, b: Rect): boolean {
  return a.x < b.x + b.w && a.x + a.w > b.x && a.y < b.y + b.h && a.y + a.h > b.y;
}
