import assert from "node:assert/strict";
import test from "node:test";
import { dockToolbar } from "./dock.ts";

function inside(dock: { left: number; top: number; width: number; height: number; subTop: number }, viewW: number, viewH: number, subH: number) {
  assert.ok(dock.left >= 8);
  assert.ok(dock.top >= 8);
  assert.ok(dock.left + dock.width <= viewW - 8);
  assert.ok(dock.top + dock.height <= viewH - 8);
  if (subH > 0) {
    assert.ok(dock.subTop >= 8);
    assert.ok(dock.subTop + subH <= viewH - 8);
  }
}

test("toolbar sits under a selection that has room below", () => {
  const dock = dockToolbar(100, 80, 400, 200, 1920, 1080, 640, 44, 0);
  assert.equal(dock.top, 80 + 200 + 10);
  assert.equal(dock.left, 8);
  inside(dock, 1920, 1080, 0);
});

test("toolbar moves above a selection that reaches the bottom", () => {
  const dock = dockToolbar(100, 900, 400, 150, 1920, 1080, 640, 44, 0);
  assert.equal(dock.top, 900 - 10 - 44);
  inside(dock, 1920, 1080, 0);
});

test("fullscreen selection keeps the toolbar inside the bottom edge", () => {
  const dock = dockToolbar(0, 0, 1920, 1080, 1920, 1080, 720, 44, 40);
  assert.equal(dock.top, 1080 - 8 - 44);
  assert.equal(dock.left, (1920 - 720) / 2);
  inside(dock, 1920, 1080, 40);
});

test("a selection on the right edge cannot push the toolbar off", () => {
  const dock = dockToolbar(1700, 200, 200, 200, 1920, 1080, 640, 44, 0);
  assert.equal(dock.left, 1920 - 8 - 640);
  inside(dock, 1920, 1080, 0);
});

test("a bar wider than the view is capped and stays on screen", () => {
  const dock = dockToolbar(0, 0, 800, 400, 800, 600, 2000, 44, 80);
  assert.equal(dock.width, 800 - 16);
  assert.equal(dock.left, 8);
  inside(dock, 800, 600, 80);
});
