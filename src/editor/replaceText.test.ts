import assert from "node:assert/strict";
import test from "node:test";
import { hasChinese, parseNumberedTranslation, parseSeenLines, replacementLayers } from "./replaceText.ts";

test("english echoed by the model is not treated as a translation", () => {
  assert.equal(hasChinese(["Schema diff — compare structures across connections"]), false);
  assert.equal(hasChinese(["架构差异 — 比较【连接】之间的结构"]), true);
});

test("vision lines become pixel boxes", () => {
  const lines = parseSeenLines("120,40,480,90|支持【自动补全】\nnot a line", 1000, 500);
  assert.equal(lines.length, 1);
  assert.equal(lines[0]?.text, "支持【自动补全】");
  assert.deepEqual(
    [lines[0]?.x, lines[0]?.y, lines[0]?.w, lines[0]?.h],
    [120, 20, 360, 25],
  );
});

test("numbered screenshot lines stay in order", () => {
  const lines = parseNumberedTranslation("1. 设置\n2. 保存", 2);
  assert.deepEqual(lines, ["设置", "保存"]);
});

test("one translation uses one font size", () => {
  const layers = replacementLayers(
    [
      { text: "Save", x: 0, y: 0, w: 80, h: 16 },
      { text: "Title", x: 0, y: 40, w: 120, h: 36 },
      { text: "A longer label", x: 0, y: 90, w: 40, h: 18 },
    ],
    ["保存", "标题", "一段更长的说明"],
    0,
    0,
    null,
    200,
    140,
  );
  const sizes = layers.filter((layer) => layer.type === "text").map((layer) => layer.type === "text" ? layer.size : 0);
  assert.deepEqual(sizes, [15, 15, 15]);
});

test("key terms marked by the model are drawn in a second color", () => {
  const layers = replacementLayers(
    [{ text: "Autocomplete", x: 0, y: 0, w: 200, h: 16 }],
    ["支持【自动补全】和运行"],
    0,
    0,
    null,
    400,
    40,
  );
  const text = layers.filter((layer) => layer.type === "text");
  assert.deepEqual(text.map((layer) => layer.type === "text" ? layer.text : ""), ["支持", "自动补全", "和运行"]);
  assert.deepEqual(text.map((layer) => layer.type === "text" ? layer.color : ""), ["#111111", "#c2410c", "#111111"]);
});

test("colored antialiasing around black type stays black", () => {
  const pixels = new Uint8ClampedArray(10 * 8 * 4);
  pixels.fill(255);
  const paint = (x: number, y: number, red: number, green: number, blue: number) => {
    const index = (y * 10 + x) * 4;
    pixels[index] = red;
    pixels[index + 1] = green;
    pixels[index + 2] = blue;
  };
  for (let x = 2; x <= 7; x += 1) paint(x, 3, 0, 0, 0);
  for (let x = 2; x <= 7; x += 1) paint(x, 2, 40, 180, 40);
  for (let x = 2; x <= 7; x += 1) paint(x, 4, 180, 40, 40);
  const layers = replacementLayers(
    [{ text: "Save", x: 1, y: 1, w: 8, h: 5 }],
    ["保存"],
    0,
    0,
    pixels,
    10,
    8,
  );
  assert.equal(layers[1]?.type === "text" ? layers[1].color : "", "#000000");
});

test("a white label with black ink is covered in those colors", () => {
  const pixels = new Uint8ClampedArray(8 * 8 * 4);
  pixels.fill(255);
  for (let y = 3; y <= 4; y += 1) {
    for (let x = 2; x <= 5; x += 1) {
      const index = (y * 8 + x) * 4;
      pixels[index] = 0;
      pixels[index + 1] = 0;
      pixels[index + 2] = 0;
    }
  }
  const layers = replacementLayers(
    [{ text: "Save", x: 1, y: 1, w: 5, h: 5 }],
    ["保存"],
    0,
    0,
    pixels,
    8,
    8,
  );
  assert.equal(layers[0]?.type, "rect");
  assert.equal(layers[0]?.type === "rect" ? layers[0].color : "", "#ffffff");
  assert.equal(layers[1]?.type, "text");
  assert.equal(layers[1]?.type === "text" ? layers[1].text : "", "保存");
  assert.equal(layers[1]?.type === "text" ? layers[1].color : "", "#000000");
});
