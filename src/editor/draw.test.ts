import assert from "node:assert/strict";
import test from "node:test";
import { highlightFill, previewScale, screenCssSize } from "./draw.ts";

test("highlight uses the chosen color at marker opacity", () => {
  assert.equal(highlightFill("#ef4444"), "rgba(239, 68, 68, 0.38)");
  assert.equal(highlightFill("#3b82f6"), "rgba(59, 130, 246, 0.38)");
});

test("a highlight saved before colors existed stays yellow", () => {
  assert.equal(highlightFill(undefined), "rgba(250, 204, 21, 0.38)");
});

test("a screen-sized selection stays at the captured resolution", () => {
  assert.equal(previewScale(1920, 1080, 1920, 1080, 1), 1);
  assert.equal(previewScale(2880, 1800, 1920, 1200, 1.5), 1);
});

test("preview matches the screen when the window is smaller than the photo", () => {
  assert.equal(previewScale(3840, 2160, 1920, 1080, 1), 0.5);
});

test("preview never enlarges past the captured photo", () => {
  assert.equal(previewScale(800, 600, 1600, 1200, 1), 1);
});

test("unknown layout keeps the photo at full resolution", () => {
  assert.equal(previewScale(2560, 1440, 0, 0, 1.5), 1);
});

test("the frozen frame is sized so one photo pixel is one device pixel", () => {
  assert.deepEqual(screenCssSize(2880, 1800, 1.5), { width: 1920, height: 1200 });
  assert.deepEqual(screenCssSize(1920, 1080, 1), { width: 1920, height: 1080 });
});
