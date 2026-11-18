import assert from "node:assert/strict";
import test from "node:test";
import { clampRect, isDragGlitch, normalize, square } from "./selection.ts";

test("normalize keeps a drag that moves up-left", () => {
  const rect = normalize(80, 60, 20, 10);
  assert.deepEqual(rect, { x: 20, y: 10, w: 60, h: 50 });
});

test("shift-square uses the longer side and the drag direction", () => {
  const rect = square(10, 10, 40, 20);
  assert.deepEqual(rect, { x: 10, y: 10, w: 30, h: 30 });
  const up = square(10, 10, 4, 20);
  assert.equal(up.x, 0);
  assert.equal(up.w, 10);
  assert.equal(up.h, 10);
});

test("mouseup glitch at the origin is ignored, a real drag to the corner is kept", () => {
  const bounds = { w: 1920, h: 1080 };
  assert.equal(isDragGlitch({ x: 0, y: 0 }, { x: 400, y: 300 }, bounds), true);
  assert.equal(isDragGlitch({ x: 0, y: 0 }, { x: 10, y: 12 }, bounds), false);
  assert.equal(isDragGlitch({ x: 1900, y: 1000 }, { x: 400, y: 300 }, bounds), false);
  assert.equal(isDragGlitch({ x: -40, y: 10 }, { x: 20, y: 20 }, bounds), true);
});

test("clamp keeps a selection inside the desktop", () => {
  const rect = clampRect({ x: -20, y: 10, w: 100, h: 40 }, { x: 0, y: 0, w: 200, h: 80 });
  assert.deepEqual(rect, { x: 0, y: 10, w: 100, h: 40 });
});
