import assert from "node:assert/strict";
import test from "node:test";
import { hitTest, layerBounds, resizeTextLayer, type Layer } from "./model.ts";

function textLayer(text: string, size: number, x = 10, y = 30): Extract<Layer, { type: "text" }> {
  return { id: "a", type: "text", x, y, text, color: "#111", size };
}

test("text bounds follow the line height used when drawing", () => {
  const one = layerBounds(textLayer("Hi", 20));
  assert.equal(one.x, 10);
  assert.equal(one.y, 30);
  assert.equal(one.h, 20);
  assert.ok(one.w > 20);
  const two = layerBounds(textLayer("你好\n世界", 20, 0, 0));
  assert.equal(two.h, 45);
  assert.ok(two.w >= 40);
});

test("south-east drag scales the font from the top left", () => {
  const layer = textLayer("Hi", 20);
  const old = layerBounds(layer);
  const next = resizeTextLayer(layer, { ...old, h: old.h * 2, w: old.w * 2 }, "se");
  assert.equal(next.size, 40);
  assert.equal(next.x, 10);
  assert.equal(next.y, 30);
});

test("north-west drag keeps the bottom-right corner", () => {
  const layer = textLayer("Hi", 20);
  const old = layerBounds(layer);
  const next = resizeTextLayer(layer, { x: old.x, y: old.y - old.h, w: old.w, h: old.h * 2 }, "nw");
  assert.equal(next.size, 40);
  const box = layerBounds(next);
  assert.equal(box.x + box.w, old.x + old.w);
  assert.equal(box.y + box.h, old.y + old.h);
});

test("a click on the glyphs selects the text", () => {
  const layer = textLayer("你好", 28, 40, 50);
  const box = layerBounds(layer);
  assert.equal(hitTest([layer], box.x + 4, box.y + 4), "a");
  assert.equal(hitTest([layer], box.x - 40, box.y), null);
});
