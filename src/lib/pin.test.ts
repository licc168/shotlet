import assert from "node:assert/strict";
import test from "node:test";
import { centeredOrigin, fittedSize, isDoubleClick, nextScale, pinWindowSize } from "./pin.ts";

test("fitted size keeps the aspect and stays inside the pin window", () => {
  const wide = fittedSize(800, 400, 420, 280);
  assert.equal(wide.w, 420);
  assert.equal(wide.h, 210);
  const tall = fittedSize(100, 400, 420, 280);
  assert.equal(tall.w, 70);
  assert.equal(tall.h, 280);
});

test("zoom grows the window instead of clipping the image", () => {
  const base = pinWindowSize(800, 400, 1);
  const zoomed = pinWindowSize(800, 400, 2);
  assert.equal(base.w, 420);
  assert.equal(base.h, 210);
  assert.equal(zoomed.w, 840);
  assert.equal(zoomed.h, 420);
  assert.deepEqual(pinWindowSize(800, 400, 8), pinWindowSize(800, 400, 4));
});

test("resize keeps the window centered", () => {
  const next = centeredOrigin(100, 80, 200, 100, 400, 200);
  assert.equal(next.x, 0);
  assert.equal(next.y, 30);
});

test("a second click close is detected even when startDragging ate the dblclick event", () => {
  const first = { time: 1000, x: 12, y: 20 };
  assert.equal(isDoubleClick(null, first), false);
  assert.equal(isDoubleClick(first, { time: 1250, x: 14, y: 22 }), true);
  assert.equal(isDoubleClick(first, { time: 1250, x: 40, y: 20 }), false);
  assert.equal(isDoubleClick(first, { time: 1600, x: 12, y: 20 }), false);
});

test("wheel steps stay within the scale range", () => {
  assert.equal(nextScale(1, true), 1.08);
  assert.equal(nextScale(0.2, false), 0.2);
  assert.equal(nextScale(3.96, true), 4);
});
