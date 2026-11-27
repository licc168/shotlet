import { uid, type Layer } from "./model.ts";

export type LineBox = { text: string; x: number; y: number; w: number; h: number };

/** Boxes are 0–1000 across the screenshot the model saw. `width` and `height` are that image, in pixels. */
export function parseSeenLines(raw: string, width: number, height: number): LineBox[] {
  const text = raw.replace(/```[\w]*\n?/g, "").trim();
  const lines: LineBox[] = [];
  for (const line of text.split(/\r?\n/)) {
    const match = line.trim().match(/^(\d+)\s*,\s*(\d+)\s*,\s*(\d+)\s*,\s*(\d+)\s*[|｜]\s*(.+)$/);
    if (!match) continue;
    const left = Number(match[1]) / 1000 * width;
    const top = Number(match[2]) / 1000 * height;
    const right = Number(match[3]) / 1000 * width;
    const bottom = Number(match[4]) / 1000 * height;
    const value = match[5]?.trim() ?? "";
    if (!value || right - left < 2 || bottom - top < 2) continue;
    lines.push({ text: value, x: left, y: top, w: right - left, h: bottom - top });
  }
  return lines;
}

/** A rewrite that is still mostly Latin did not translate. */
export function hasChinese(lines: string[]): boolean {
  const text = lines.join("");
  const cjk = text.match(/[\u4e00-\u9fff]/g)?.length ?? 0;
  const latin = text.match(/[A-Za-z]/g)?.length ?? 0;
  return cjk >= 4 && cjk * 2 >= latin;
}

export function plainTranslation(lines: string[]): string {
  return lines.map((line) => line.replace(/[【】]/g, "")).join("\n");
}

export function parseNumberedTranslation(raw: string, count: number): string[] | null {
  if (count <= 0) return [];
  const text = raw.replace(/```[\w]*\n?/g, "").trim();
  const found = new Map<number, string>();
  for (const line of text.split(/\r?\n/)) {
    const match = line.match(/^\s*(\d+)\s*[\.\)、:：]\s*(.+?)\s*$/);
    if (!match) continue;
    found.set(Number(match[1]), match[2].trim());
  }
  if (found.size !== count) return null;
  const lines: string[] = [];
  for (let index = 1; index <= count; index += 1) {
    const line = found.get(index);
    if (!line) return null;
    lines.push(line);
  }
  return lines;
}

export function replacementLayers(
  lines: LineBox[],
  translated: string[],
  originX: number,
  originY: number,
  pixels: Uint8ClampedArray | null,
  width: number,
  height: number,
): Layer[] {
  const count = Math.min(lines.length, translated.length);
  const boxes: Array<{ text: string; box: { x: number; y: number; w: number; h: number } }> = [];
  for (let index = 0; index < count; index += 1) {
    const text = translated[index].trim();
    if (!text) continue;
    const line = lines[index];
    const box = snap(clip(originX + line.x, originY + line.y, line.w, line.h, width, height));
    if (box.w < 2 || box.h < 2) continue;
    boxes.push({ text, box });
  }
  const size = sharedSize(boxes.map((item) => item.box.h));
  const layers: Layer[] = [];
  for (const item of boxes) {
    const { text, box } = item;
    const { bg, fg } = pixels ? samplePatch(pixels, width, height, box) : { bg: "#ffffff", fg: "#111111" };
    const marked = emphasisColor(bg);
    layers.push({ id: uid(), type: "rect", x: box.x, y: box.y, w: box.w, h: box.h, color: bg, thickness: 1, filled: true });
    let x = box.x;
    const y = box.y + Math.max(0, Math.round((box.h - size) / 2));
    for (const run of emphasisRuns(text)) {
      layers.push({ id: uid(), type: "text", x, y, text: run.text, color: run.emphasis ? marked : fg, size });
      x += Math.round(textWidth(run.text, size));
    }
  }
  return layers;
}

function clip(x: number, y: number, w: number, h: number, width: number, height: number) {
  const x0 = Math.max(0, x);
  const y0 = Math.max(0, y);
  const x1 = Math.min(width, x + w);
  const y1 = Math.min(height, y + h);
  return { x: x0, y: y0, w: Math.max(0, x1 - x0), h: Math.max(0, y1 - y0) };
}

function emphasisRuns(text: string): Array<{ text: string; emphasis: boolean }> {
  const runs: Array<{ text: string; emphasis: boolean }> = [];
  const pattern = /【([^】]+)】/g;
  let cursor = 0;
  for (const match of text.matchAll(pattern)) {
    const start = match.index ?? 0;
    if (start > cursor) runs.push({ text: text.slice(cursor, start), emphasis: false });
    if (match[1]) runs.push({ text: match[1], emphasis: true });
    cursor = start + match[0].length;
  }
  if (cursor < text.length) runs.push({ text: text.slice(cursor), emphasis: false });
  return runs.filter((run) => run.text.length > 0);
}

function emphasisColor(bg: string): string {
  const background = rgb(bg);
  return luminance(background) > 160 ? "#c2410c" : "#fbbf24";
}

function rgb(value: string): Rgb {
  const match = /^#([0-9a-f]{6})$/i.exec(value);
  if (!match) return [17, 17, 17];
  const hex = match[1] ?? "111111";
  return [Number.parseInt(hex.slice(0, 2), 16), Number.parseInt(hex.slice(2, 4), 16), Number.parseInt(hex.slice(4, 6), 16)];
}

function textWidth(text: string, size: number): number {
  let width = 0;
  for (const ch of text) {
    const code = ch.codePointAt(0) ?? 0;
    width += code > 255 ? size : size * 0.56;
  }
  return width;
}

function snap(box: { x: number; y: number; w: number; h: number }) {
  const x = Math.round(box.x);
  const y = Math.round(box.y);
  return { x, y, w: Math.max(0, Math.round(box.x + box.w) - x), h: Math.max(0, Math.round(box.y + box.h) - y) };
}

/** One translation uses one size: the median line, not each box. */
function sharedSize(heights: number[]): number {
  if (heights.length === 0) return 12;
  const sizes = heights.map(typeSize).sort((left, right) => left - right);
  return sizes[Math.floor(sizes.length / 2)] ?? 12;
}

function typeSize(boxH: number): number {
  const bucket = Math.max(8, Math.floor(boxH / 4) * 4);
  return Math.max(8, Math.round(bucket * 0.92));
}

function samplePatch(
  data: Uint8ClampedArray,
  width: number,
  height: number,
  box: { x: number; y: number; w: number; h: number },
): { bg: string; fg: string } {
  const x0 = Math.max(0, Math.min(width - 1, Math.floor(box.x)));
  const y0 = Math.max(0, Math.min(height - 1, Math.floor(box.y)));
  const x1 = Math.max(x0 + 1, Math.min(width, Math.ceil(box.x + box.w)));
  const y1 = Math.max(y0 + 1, Math.min(height, Math.ceil(box.y + box.h)));
  const at = (x: number, y: number): Rgb => {
    const index = (y * width + x) * 4;
    return [data[index] ?? 255, data[index + 1] ?? 255, data[index + 2] ?? 255];
  };
  const edges: Rgb[] = [];
  for (let x = x0; x < x1; x += 1) {
    edges.push(at(x, y0));
    if (y1 - 1 !== y0) edges.push(at(x, y1 - 1));
  }
  for (let y = y0 + 1; y < y1 - 1; y += 1) {
    edges.push(at(x0, y));
    if (x1 - 1 !== x0) edges.push(at(x1 - 1, y));
  }
  const bg = medianColor(edges);
  const inks: Rgb[] = [];
  for (let y = y0; y < y1; y += 1) {
    for (let x = x0; x < x1; x += 1) {
      const pixel = at(x, y);
      if (distance(pixel, bg) > 80) inks.push(pixel);
    }
  }
  const fg = inkColor(inks, bg);
  return { bg: hex(bg), fg: hex(fg) };
}

/** ClearType leaves red and green fringes around black type. The darkest quarter is the stroke itself. */
function inkColor(pixels: Rgb[], bg: Rgb): Rgb {
  const ranked = pixels.slice().sort((left, right) => luminance(left) - luminance(right));
  if (ranked.length === 0) return contrast(bg);
  const keep = Math.max(1, Math.ceil(ranked.length * 0.25));
  return medianColor(ranked.slice(0, keep));
}

type Rgb = [number, number, number];

function medianColor(pixels: Rgb[]): Rgb {
  if (pixels.length === 0) return [255, 255, 255];
  return [median(pixels.map((pixel) => pixel[0])), median(pixels.map((pixel) => pixel[1])), median(pixels.map((pixel) => pixel[2]))];
}

function median(values: number[]): number {
  const sorted = [...values].sort((left, right) => left - right);
  return sorted[Math.floor(sorted.length / 2)] ?? 255;
}

function distance(left: Rgb, right: Rgb): number {
  return Math.abs(left[0] - right[0]) + Math.abs(left[1] - right[1]) + Math.abs(left[2] - right[2]);
}

function luminance(color: Rgb): number {
  return 0.2126 * color[0] + 0.7152 * color[1] + 0.0722 * color[2];
}

function contrast(color: Rgb): Rgb {
  return luminance(color) > 160 ? [17, 17, 17] : [255, 255, 255];
}

function hex(color: Rgb): string {
  return `#${color.map((channel) => channel.toString(16).padStart(2, "0")).join("")}`;
}
