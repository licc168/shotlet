export const PIN_BASE_WIDTH = 420;
export const PIN_BASE_HEIGHT = 280;
export const PIN_MIN_SCALE = 0.2;
export const PIN_MAX_SCALE = 4;
export const PIN_SCALE_STEP = 0.08;
export const PIN_MIN_OPACITY = 0.15;
export const PIN_MAX_OPACITY = 1;
export const PIN_OPACITY_STEP = 0.05;
export const PIN_DOUBLE_CLICK_MS = 400;
export const PIN_DOUBLE_CLICK_DIST = 8;

export type Size = { w: number; h: number };
export type Point = { x: number; y: number };
export type ClickStamp = { time: number; x: number; y: number };

export function clampScale(value: number): number {
  return Math.min(PIN_MAX_SCALE, Math.max(PIN_MIN_SCALE, value));
}

export function nextScale(value: number, zoomIn: boolean): number {
  return clampScale(value + (zoomIn ? PIN_SCALE_STEP : -PIN_SCALE_STEP));
}

/** Contain-fit a natural image size into the pin window, like object-fit: contain. */
export function fittedSize(naturalW: number, naturalH: number, boxW = PIN_BASE_WIDTH, boxH = PIN_BASE_HEIGHT): Size {
  const width = Math.max(1, naturalW);
  const height = Math.max(1, naturalH);
  const scale = Math.min(boxW / width, boxH / height);
  return {
    w: Math.max(1, Math.round(width * scale)),
    h: Math.max(1, Math.round(height * scale)),
  };
}

/** Window size that keeps the whole pinned image visible at the given zoom. */
export function pinWindowSize(naturalW: number, naturalH: number, scale: number, boxW = PIN_BASE_WIDTH, boxH = PIN_BASE_HEIGHT): Size {
  const fitted = fittedSize(naturalW, naturalH, boxW, boxH);
  const zoom = clampScale(scale);
  return {
    w: Math.max(1, Math.round(fitted.w * zoom)),
    h: Math.max(1, Math.round(fitted.h * zoom)),
  };
}

/** New top-left so the window stays centered when its size changes. */
export function centeredOrigin(x: number, y: number, oldW: number, oldH: number, newW: number, newH: number): Point {
  return {
    x: x - (newW - oldW) / 2,
    y: y - (newH - oldH) / 2,
  };
}

export function isDoubleClick(prev: ClickStamp | null, next: ClickStamp, maxMs = PIN_DOUBLE_CLICK_MS, maxDist = PIN_DOUBLE_CLICK_DIST): boolean {
  if (!prev) return false;
  const dt = next.time - prev.time;
  if (dt < 0 || dt > maxMs) return false;
  const dx = next.x - prev.x;
  const dy = next.y - prev.y;
  return dx * dx + dy * dy <= maxDist * maxDist;
}
