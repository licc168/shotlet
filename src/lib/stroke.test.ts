import assert from "node:assert/strict";
import test from "node:test";
import { appendStrokePoint } from "./stroke.ts";

test("brush points follow the pointer instead of collapsing to a straight line", () => {
  const start = { points: [{ x: 10, y: 10 }] };
  const bent = [
    { x: 40, y: 12 },
    { x: 48, y: 36 },
    { x: 20, y: 40 },
  ].reduce((stroke, point) => appendStrokePoint(stroke, point), start);
  assert.deepEqual(bent.points, [
    { x: 10, y: 10 },
    { x: 40, y: 12 },
    { x: 48, y: 36 },
    { x: 20, y: 40 },
  ]);
});

test("brush ignores jitter closer than the step", () => {
  const stroke = appendStrokePoint({ points: [{ x: 10, y: 10 }] }, { x: 10.4, y: 10.2 });
  assert.equal(stroke.points.length, 1);
});
