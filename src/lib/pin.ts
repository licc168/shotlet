export const PIN_BASE_WIDTH = 420;
export const PIN_BASE_HEIGHT = 280;
export const PIN_MIN_SCALE = 0.2;
export const PIN_MAX_SCALE = 4;
export const PIN_SCALE_STEP = 0.025;
export const PIN_MIN_OPACITY = 0.15;
export const PIN_MAX_OPACITY = 1;
export const PIN_OPACITY_STEP = 0.05;
export const PIN_DOUBLE_CLICK_MS = 400;
export const PIN_DOUBLE_CLICK_DIST = 8;
/** Physical pixels of transparent margin around the cut. Must match PIN_GLOW in src-tauri/src/ui.rs. */
export const PIN_GLOW = 22;

export type Size = { w: number; h: number };
export type Point = { x: number; y: number };
export type ClickStamp = { time: number; x: number; y: number };

export function clampScale(value: number): number {
  return Math.min(PIN_MAX_SCALE, Math.max(PIN_MIN_SCALE, value));
}

/** One wheel tick changes size by a small ratio, same at every zoom. */
export function nextScale(value: number, zoomIn: boolean): number {
  const factor = zoomIn ? 1 + PIN_SCALE_STEP : 1 / (1 + PIN_SCALE_STEP);
  return clampScale(value * factor);
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

/**
 * Window size for a cut-out pin. Scale 1 is one image pixel per screen pixel,
 * so the sticker keeps the selection's on-screen size instead of shrinking
 * into a fixed box.
 */
export function pinWindowSize(naturalW: number, naturalH: number, scale: number): Size {
  const zoom = clampScale(scale);
  return {
    w: Math.max(1, Math.round(Math.max(1, naturalW) * zoom)),
    h: Math.max(1, Math.round(Math.max(1, naturalH) * zoom)),
  };
}

/** Window around the cut: picture size plus a fixed glow margin on every side. */
export function pinFrameSize(imageW: number, imageH: number, glow = PIN_GLOW): Size {
  return {
    w: Math.max(1, Math.round(imageW)) + glow * 2,
    h: Math.max(1, Math.round(imageH)) + glow * 2,
  };
}

/** Top-left of the window so the picture itself stays on (x, y). */
export function pinFrameOrigin(x: number, y: number, glow = PIN_GLOW): Point {
  return { x: x - glow, y: y - glow };
}

/** CSS pixels that equal PIN_GLOW physical pixels at the current screen scale. */
export function pinGlowCssPx(devicePixelRatio = 1): number {
  const ratio = Number.isFinite(devicePixelRatio) && devicePixelRatio > 0 ? devicePixelRatio : 1;
  return PIN_GLOW / ratio;
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
