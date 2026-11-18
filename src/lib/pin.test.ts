import assert from "node:assert/strict";
import test from "node:test";
import { PIN_GLOW, centeredOrigin, fittedSize, isDoubleClick, nextScale, pinFrameOrigin, pinFrameSize, pinGlowCssPx, pinWindowSize } from "./pin.ts";

test("fitted size keeps the aspect and stays inside the pin window", () => {
  const wide = fittedSize(800, 400, 420, 280);
  assert.equal(wide.w, 420);
  assert.equal(wide.h, 210);
  const tall = fittedSize(100, 400, 420, 280);
  assert.equal(tall.w, 70);
  assert.equal(tall.h, 280);
});

test("scale 1 keeps the cut size and zoom grows from that", () => {
  const base = pinWindowSize(800, 400, 1);
  const zoomed = pinWindowSize(800, 400, 2);
  assert.equal(base.w, 800);
  assert.equal(base.h, 400);
  assert.equal(zoomed.w, 1600);
  assert.equal(zoomed.h, 800);
  assert.deepEqual(pinWindowSize(543, 704, 1), { w: 543, h: 704 });
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
  const up = nextScale(1, true);
  assert.ok(Math.abs(up - 1.025) < 1e-9);
  assert.equal(nextScale(0.2, false), 0.2);
  assert.equal(nextScale(3.96, true), 4);
  assert.ok(nextScale(1, false) > 0.97 && nextScale(1, false) < 1);
});

test("glow margin sits outside the cut and does not scale", () => {
  assert.equal(PIN_GLOW, 22);
  assert.deepEqual(pinFrameSize(526, 681), { w: 570, h: 725 });
  assert.deepEqual(pinFrameOrigin(713, 178), { x: 691, y: 156 });
  const zoomed = pinWindowSize(526, 681, 2);
  assert.deepEqual(pinFrameSize(zoomed.w, zoomed.h), { w: 526 * 2 + PIN_GLOW * 2, h: 681 * 2 + PIN_GLOW * 2 });
  assert.equal(pinGlowCssPx(1), 22);
  assert.equal(pinGlowCssPx(2), 11);
});

test("zoom stays centered when the glow margin is part of the window", () => {
  const x = 713;
  const y = 178;
  const w = 526;
  const h = 681;
  const origin = pinFrameOrigin(x, y);
  const before = pinFrameSize(w, h);
  const zoomed = pinWindowSize(w, h, 2);
  const after = pinFrameSize(zoomed.w, zoomed.h);
  const next = centeredOrigin(origin.x, origin.y, before.w, before.h, after.w, after.h);
  assert.equal(next.x + PIN_GLOW + zoomed.w / 2, x + w / 2);
  assert.equal(next.y + PIN_GLOW + zoomed.h / 2, y + h / 2);
});
