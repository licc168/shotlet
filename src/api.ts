import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import { defaultSettings, type CommitBody, type EditorPayload, type FrozenCapture, type HistoryItem, type Settings } from "./types";

/** A decoded screenshot. An image element or a canvas that already holds the captured pixels. */
export type ShotImage = CanvasImageSource & {
  naturalWidth: number;
  naturalHeight: number;
  src: string;
  /** Raw RGBA when available (binary IPC path), so colour sampling is O(1) and GPU-free. */
  pixels?: Uint8ClampedArray<ArrayBuffer>;
};

const SETTINGS_KEY = "shotlet.settings";
const HISTORY_KEY = "shotlet.history";

export function isTauri(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

export function imageSrc(path: string): string {
  if (!path || path.startsWith("data:") || path.startsWith("blob:") || path.startsWith("http")) return path;
  return isTauri() ? convertFileSrc(path) : path;
}

async function call<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  return invoke<T>(command, args);
}

export async function onEvent<T>(name: string, handler: (payload: T) => void): Promise<() => void> {
  if (!isTauri()) return () => undefined;
  const { listen } = await import("@tauri-apps/api/event");
  const unlisten = await listen<T>(name, (event) => handler(event.payload));
  return () => {
    unlisten();
  };
}

function readSettings(): Settings {
  const raw = localStorage.getItem(SETTINGS_KEY);
  if (!raw) return defaultSettings;
  const saved = JSON.parse(raw) as Partial<Settings>;
  const merged = { ...defaultSettings, ...saved };
  if (!("pinShortcut" in saved) && saved.captureShortcut === "Super+Shift+KeyA") {
    merged.captureShortcut = "F1";
  }
  return merged;
}

function readHistory(): HistoryItem[] {
  const raw = localStorage.getItem(HISTORY_KEY);
  if (!raw) return [];
  return JSON.parse(raw) as HistoryItem[];
}

function writeHistory(items: HistoryItem[]) {
  localStorage.setItem(HISTORY_KEY, JSON.stringify(items));
}

export async function getSettings(): Promise<Settings> {
  if (!isTauri()) return readSettings();
  return call<Settings>("get_settings");
}

export async function saveSettings(settings: Settings): Promise<void> {
  if (!isTauri()) {
    localStorage.setItem(SETTINGS_KEY, JSON.stringify(settings));
    return;
  }
  await call("save_settings", { settings });
}

export async function startCapture(mode = "edit"): Promise<void> {
  if (!isTauri()) {
    const query = mode === "edit" ? "view=overlay" : `view=overlay&mode=${encodeURIComponent(mode)}`;
    window.location.assign(`/?${query}`);
    return;
  }
  await call("start_capture", { mode });
}

export async function copyCaptureRegion(x: number, y: number, width: number, height: number): Promise<void> {
  await call("copy_capture_region", { x, y, width, height });
}

export async function overlayReady(generation: number, cursor = true): Promise<void> {
  if (!isTauri()) return;
  await call("overlay_ready", { generation, cursor });
}

export async function setOverlayFollow(follow: boolean): Promise<void> {
  if (!isTauri()) return;
  await call("set_overlay_follow", { follow });
}

export async function aimOverlay(x: number, y: number): Promise<void> {
  if (!isTauri()) return;
  await call("aim_overlay", { x, y });
}

export async function currentCapture(): Promise<FrozenCapture | null> {
  if (!isTauri()) return demoCapture();
  return call<FrozenCapture | null>("current_capture");
}

export async function cancelCapture(): Promise<void> {
  if (!isTauri()) {
    window.location.assign("/");
    return;
  }
  await call("cancel_capture");
}

export async function listHistory(query: string, filter: string): Promise<HistoryItem[]> {
  if (!isTauri()) {
    const needle = query.trim().toLowerCase();
    return readHistory().filter((item) => {
      const deleted = item.deleted;
      if (filter === "deleted" && !deleted) return false;
      if (filter === "favorite" && (!item.favorite || deleted)) return false;
      if (filter !== "deleted" && filter !== "favorite" && deleted) return false;
      if (!needle) return true;
      return item.name.toLowerCase().includes(needle)
        || item.ocrText.toLowerCase().includes(needle)
        || item.tags.join(" ").toLowerCase().includes(needle);
    });
  }
  return call<HistoryItem[]>("list_history", { query, filter });
}

export async function commitHistory(request: CommitBody): Promise<HistoryItem> {
  if (!isTauri()) {
    const item: HistoryItem = {
      id: String(Date.now()),
      createdAt: Date.now(),
      name: request.name || "截图",
      tags: [],
      favorite: false,
      width: request.width,
      height: request.height,
      ocrText: request.ocrText,
      file: `data:image/png;base64,${request.pngBase64}`,
      deleted: false,
    };
    writeHistory([item, ...readHistory()].slice(0, 24));
    sessionStorage.setItem(`shotlet-doc-${item.id}`, JSON.stringify({
      base: `data:image/png;base64,${request.basePngBase64}`,
      document: request.document,
      width: request.width,
      height: request.height,
    }));
    return item;
  }
  return call<HistoryItem>("commit_history", { request });
}

export async function updateHistory(id: string, request: CommitBody): Promise<HistoryItem> {
  if (!isTauri()) {
    const items = readHistory();
    const index = items.findIndex((item) => item.id === id);
    if (index < 0) throw new Error("missing");
    items[index] = {
      ...items[index],
      name: request.name || items[index].name,
      width: request.width,
      height: request.height,
      ocrText: request.ocrText,
      file: `data:image/png;base64,${request.pngBase64}`,
    };
    writeHistory(items);
    sessionStorage.setItem(`shotlet-doc-${id}`, JSON.stringify({
      base: `data:image/png;base64,${request.basePngBase64}`,
      document: request.document,
      width: request.width,
      height: request.height,
    }));
    return items[index];
  }
  return call<HistoryItem>("update_history", { id, request });
}

/** Metadata carried in the binary commit frame; the images travel as raw bytes. */
type CommitMeta = {
  document: string;
  ocrText: string;
  width: number;
  height: number;
  name: string;
  id?: string;
};

/**
 * A raw IPC body can hold only one blob, so pack both PNGs and the metadata
 * into a single self-describing frame:
 *   [u32 png len][u32 base len][u32 meta len][png][base][meta JSON]
 */
function commitFrame(meta: CommitMeta, png: Uint8Array<ArrayBuffer>, base: Uint8Array<ArrayBuffer>): Uint8Array<ArrayBuffer> {
  const metaBytes = new TextEncoder().encode(JSON.stringify(meta));
  const out = new Uint8Array(12 + png.length + base.length + metaBytes.length);
  const view = new DataView(out.buffer);
  view.setUint32(0, png.length, true);
  view.setUint32(4, base.length, true);
  view.setUint32(8, metaBytes.length, true);
  out.set(png, 12);
  out.set(base, 12 + png.length);
  out.set(metaBytes, 12 + png.length + base.length);
  return out;
}

function commitMeta(request: CommitBody): CommitMeta {
  return {
    document: request.document,
    ocrText: request.ocrText,
    width: request.width,
    height: request.height,
    name: request.name,
  };
}

/** Commit with raw PNG bytes. Browser demos fall back to the Base64 request. */
export async function commitHistoryRaw(
  request: CommitBody,
  png: Uint8Array<ArrayBuffer>,
  base: Uint8Array<ArrayBuffer>,
): Promise<HistoryItem> {
  if (!isTauri()) return commitHistory(request);
  return invoke<HistoryItem>("commit_history_raw", commitFrame(commitMeta(request), png, base));
}

export async function updateHistoryRaw(
  id: string,
  request: CommitBody,
  png: Uint8Array<ArrayBuffer>,
  base: Uint8Array<ArrayBuffer>,
): Promise<HistoryItem> {
  if (!isTauri()) return updateHistory(id, request);
  return invoke<HistoryItem>("update_history_raw", commitFrame({ ...commitMeta(request), id }, png, base));
}

async function patch(id: string, change: (item: HistoryItem) => void): Promise<void> {
  const items = readHistory();
  const item = items.find((entry) => entry.id === id);
  if (!item) return;
  change(item);
  writeHistory(items);
}

export async function setFavorite(id: string, favorite: boolean): Promise<void> {
  if (!isTauri()) return patch(id, (item) => { item.favorite = favorite; });
  await call("set_favorite", { id, favorite });
}

export async function setTags(id: string, tags: string[]): Promise<void> {
  if (!isTauri()) return patch(id, (item) => { item.tags = tags; });
  await call("set_tags", { id, tags });
}

export async function renameCapture(id: string, name: string): Promise<void> {
  if (!isTauri()) return patch(id, (item) => { item.name = name; });
  await call("rename_capture", { id, name });
}

export async function deleteCaptures(ids: string[]): Promise<void> {
  if (!isTauri()) {
    const items = readHistory().map((item) => ids.includes(item.id) ? { ...item, deleted: true } : item);
    writeHistory(items);
    return;
  }
  await call("delete_captures", { ids });
}

export async function restoreCaptures(ids: string[]): Promise<void> {
  if (!isTauri()) {
    writeHistory(readHistory().map((item) => ids.includes(item.id) ? { ...item, deleted: false } : item));
    return;
  }
  await call("restore_captures", { ids });
}

export async function purgeDeleted(): Promise<void> {
  if (!isTauri()) {
    writeHistory(readHistory().filter((item) => !item.deleted));
    return;
  }
  await call("purge_deleted");
}

export async function exportCaptures(ids: string[], destDir: string): Promise<number> {
  if (!isTauri()) return ids.length;
  return call<number>("export_captures", { ids, destDir });
}

export async function copyFile(path: string): Promise<void> {
  if (!isTauri()) return copyPngBase64(await fileToBase64(path));
  await call("copy_file", { path });
}

/** Copy an annotated shot with raw PNG bytes; no Base64 encode or decode. */
export async function copyAnnotatedRaw(
  png: string,
  document: string,
  pngBytes: Uint8Array<ArrayBuffer>,
  baseBytes: Uint8Array<ArrayBuffer>,
): Promise<void> {
  if (!isTauri()) {
    await copyPngBase64(png);
    return;
  }
  await invoke("copy_annotated_raw", commitFrame({ document, ocrText: "", width: 0, height: 0, name: "" }, pngBytes, baseBytes));
}

export async function copyPngRaw(data: string, bytes: Uint8Array<ArrayBuffer>): Promise<void> {
  if (!isTauri()) {
    await copyPngBase64(data);
    return;
  }
  await invoke("copy_png_raw", bytes);
}

export async function copyAnnotated(png: string, base: string, document: string): Promise<void> {
  if (!isTauri()) {
    await copyPngBase64(png);
    return;
  }
  await call("copy_annotated", { png, base, document });
}

export async function copyPngBase64(data: string): Promise<void> {
  if (!isTauri()) {
    const blob = await (await fetch(`data:image/png;base64,${data}`)).blob();
    await navigator.clipboard.write([new ClipboardItem({ "image/png": blob })]);
    return;
  }
  await call("copy_png_base64", { data });
}

export async function writePngBase64(data: string, dest: string): Promise<void> {
  if (!isTauri()) {
    const link = document.createElement("a");
    link.href = `data:image/png;base64,${data}`;
    link.download = dest || "shotlet.png";
    link.click();
    return;
  }
  await call("write_png_base64", { data, dest });
}

export async function pinPngBase64(data: string, x: number, y: number): Promise<void> {
  tracePage("pin: click");
  if (!isTauri()) {
    sessionStorage.setItem("shotlet-pin", `data:image/png;base64,${data}`);
    window.open("/?view=pin&id=browser", "shotlet-pin", "popup,width=480,height=360");
    return;
  }
  await call("pin_png_base64", { data, x, y });
}

export async function pinCaptureRegion(
  x: number,
  y: number,
  width: number,
  height: number,
  screenX: number,
  screenY: number,
  localX: number,
  localY: number,
  bitmapW: number,
  bitmapH: number,
  pointW: number,
  pointH: number,
): Promise<void> {
  tracePage("pin: plain click");
  if (!isTauri()) return;
  await call("pin_capture_region", { x, y, width, height, screenX, screenY, localX, localY, bitmapW, bitmapH, pointW, pointH });
}

export type PinPlace = {
  x: number;
  y: number;
  width: number;
  height: number;
  screenX: number;
  screenY: number;
  localX: number;
  localY: number;
  bitmapW: number;
  bitmapH: number;
  pointW: number;
  pointH: number;
};

/** Send painted RGBA. Returns the pin id. An empty id means the base64 path already saved history. */
export async function pinAnnotatedRgba(pixels: Uint8ClampedArray<ArrayBuffer>, width: number, height: number, place: PinPlace): Promise<string> {
  tracePage("pin: annotated rgba");
  if (!isTauri()) return "";
  const body = new Uint8Array(pixels.buffer, pixels.byteOffset, pixels.byteLength);
  return invoke<string>("pin_annotated", body, {
    headers: {
      "x-pin-x": String(place.x),
      "x-pin-y": String(place.y),
      "x-pin-w": String(place.width),
      "x-pin-h": String(place.height),
      "x-pin-screen-x": String(place.screenX),
      "x-pin-screen-y": String(place.screenY),
      "x-pin-local-x": String(place.localX),
      "x-pin-local-y": String(place.localY),
      "x-pin-bitmap-w": String(place.bitmapW),
      "x-pin-bitmap-h": String(place.bitmapH),
      "x-pin-point-w": String(place.pointW),
      "x-pin-point-h": String(place.pointH),
      "x-pin-pixel-w": String(width),
      "x-pin-pixel-h": String(height),
    },
  });
}

/** Record the annotation after the sticker is already on screen. */
export async function pinHistory(id: string, document: string): Promise<void> {
  if (!isTauri() || !id) return;
  await call("pin_history", { id, document });
}

export async function pinAnnotated(
  png: string,
  base: string,
  document: string,
  screenX: number,
  screenY: number,
  localX: number,
  localY: number,
  width: number,
  height: number,
  bitmapW: number,
  bitmapH: number,
  pointW: number,
  pointH: number,
): Promise<void> {
  tracePage("pin: annotated png");
  if (!isTauri()) return pinPngBase64(png, screenX + localX, screenY + localY);
  await call("pin_annotated", {
    png,
    base,
    document,
    screenX,
    screenY,
    localX,
    localY,
    width,
    height,
    bitmapW,
    bitmapH,
    pointW,
    pointH,
  });
}

export async function pinFile(path: string, x: number, y: number): Promise<void> {
  if (!isTauri()) return pinPngBase64(await fileToBase64(path), x, y);
  await call("pin_file", { path, x, y });
}

export async function pinPath(id: string): Promise<string> {
  if (!isTauri()) return sessionStorage.getItem("shotlet-pin") ?? "";
  return call<string>("pin_path", { id });
}

/** Copy the sticker from memory while the pixels are still there, otherwise from its file. */
export async function copyPin(id: string): Promise<void> {
  if (!isTauri()) {
    const path = await pinPath(id);
    if (path) await copyFile(path);
    return;
  }
  await call("copy_pin", { id });
}

/** A warmed pin window has loaded and is waiting for a picture. */
export async function pinStandby(): Promise<void> {
  if (!isTauri()) return;
  tracePage("pin: standby");
  await call("pin_standby");
}

/** Ask the shell to resize the pin around its current center. */
export async function pinZoom(scale: number): Promise<void> {
  if (!isTauri()) return;
  await call("pin_zoom", { scale });
}

/** Tell the shell the sticker pixels are on the page, so it can reveal the window. */
export async function pinReady(): Promise<void> {
  if (!isTauri()) return;
  tracePage("pin: painted");
  await call("pin_ready");
}

export function tracePage(stage: string) {
  if (!isTauri()) return;
  const at = performance.timeOrigin + performance.now();
  void call("trace_mark", { stage, at }).catch(() => undefined);
}

export async function ocrPngBase64(data: string): Promise<string> {
  if (!isTauri()) return "Shotlet";
  return call<string>("ocr_png_base64", { data });
}

/** OCR a PNG without a Base64 round trip. The body is raw PNG bytes. */
export async function ocrPng(bytes: Uint8Array<ArrayBuffer>): Promise<string> {
  if (!isTauri()) return "Shotlet";
  return invoke<string>("ocr_png_raw", bytes);
}

function base64Utf8(text: string): string {
  const bytes = new TextEncoder().encode(text);
  let binary = "";
  for (let index = 0; index < bytes.length; index += 0x4000) {
    binary += String.fromCharCode(...bytes.subarray(index, index + 0x4000));
  }
  return btoa(binary);
}

/** Save raw PNG bytes to a path; the destination travels in a header. */
export async function writePng(bytes: Uint8Array<ArrayBuffer>, dest: string): Promise<void> {
  if (!isTauri()) {
    const url = URL.createObjectURL(new Blob([bytes], { type: "image/png" }));
    const link = document.createElement("a");
    link.href = url;
    link.download = dest || "shotlet.png";
    link.click();
    setTimeout(() => URL.revokeObjectURL(url), 10_000);
    return;
  }
  await invoke("write_png_raw", bytes, {
    headers: {
      "x-write-dest": base64Utf8(dest),
    },
  });
}

export async function openHistory(): Promise<void> {
  if (!isTauri()) {
    window.location.assign("/?view=history");
    return;
  }
  await call("open_history");
}

export async function openSettings(): Promise<void> {
  if (!isTauri()) {
    window.location.assign("/?view=settings");
    return;
  }
  await call("open_settings");
}

export async function openEditorRecord(id: string): Promise<void> {
  if (!isTauri()) {
    window.location.assign(`/?view=editor&id=${id}`);
    return;
  }
  await call("open_editor_record", { id });
}

export async function currentEditor(idFromQuery: string | null): Promise<EditorPayload | null> {
  if (!isTauri()) {
    if (!idFromQuery) {
      const raw = sessionStorage.getItem("shotlet-editor");
      return raw ? JSON.parse(raw) as EditorPayload : null;
    }
    const raw = sessionStorage.getItem(`shotlet-doc-${idFromQuery}`);
    if (!raw) return null;
    const saved = JSON.parse(raw) as { base: string; document: string; width: number; height: number };
    return { recordId: idFromQuery, path: saved.base, width: saved.width, height: saved.height, document: saved.document };
  }
  return call<EditorPayload | null>("current_editor");
}

export type ScrollRequest = {
  screenX: number;
  screenY: number;
  screenW: number;
  screenH: number;
  x: number;
  y: number;
  width: number;
  height: number;
  imageW: number;
  imageH: number;
  pointW: number;
  pointH: number;
};

export async function scrollCapture(request: ScrollRequest): Promise<void> {
  if (!isTauri()) {
    window.alert("滚动长截图需要桌面端。框住内容后，在页面里滚动。");
    return;
  }
  await call("scroll_capture", { request });
}

export async function finishScroll(action: "edit" | "save" | "copy"): Promise<void> {
  if (!isTauri()) return;
  await call("scroll_finish", { action });
}

export async function cancelScroll(): Promise<void> {
  if (!isTauri()) return;
  await call("scroll_cancel");
}

export async function scrollProgress(): Promise<{ height: number; reverse: boolean }> {
  if (!isTauri()) return { height: 0, reverse: false };
  return call("scroll_progress");
}

export async function ffmpegAvailable(): Promise<boolean> {
  if (!isTauri()) return false;
  return call<boolean>("ffmpeg_available");
}

export type RecordAudio = "none" | "mic" | "speaker" | "both";

export async function openRecording(
  screenX: number,
  screenY: number,
  localX: number,
  localY: number,
  width: number,
  height: number,
  bitmapW: number,
  bitmapH: number,
  pointW: number,
  pointH: number,
): Promise<void> {
  if (!isTauri()) {
    window.alert("区域录制需要桌面端。");
    return;
  }
  await call("open_recording", { screenX, screenY, localX, localY, width, height, bitmapW, bitmapH, pointW, pointH });
}

export async function beginRecording(format: "gif" | "mp4", audio: RecordAudio, cursor: boolean): Promise<void> {
  if (!isTauri()) return;
  await call("begin_recording", { format, audio, cursor });
}

export async function setRecordingPaused(paused: boolean): Promise<void> {
  if (!isTauri()) return;
  await call("set_recording_paused", { paused });
}

export type RecordingState = {
  running: boolean;
  paused: boolean;
  done: boolean;
  ready: boolean;
  elapsedMs: number;
  limitMs: number;
  width: number;
  height: number;
  frames: number;
  message?: string;
};

export async function recordingStatus(): Promise<RecordingState> {
  if (!isTauri()) return { running: false, paused: false, done: false, ready: true, elapsedMs: 0, limitMs: 3_600_000, width: 0, height: 0, frames: 0, message: "" };
  return call("recording_status");
}

export async function stopRecording(): Promise<string> {
  return call<string>("stop_recording");
}

export async function storageDir(): Promise<string> {
  if (!isTauri()) return "浏览器本地";
  return call<string>("storage_dir");
}

export async function revealPath(path: string): Promise<void> {
  if (!isTauri()) return;
  await call("reveal_path", { path });
}

export async function pickSavePath(name: string): Promise<string | null> {
  if (!isTauri()) return name;
  const { save } = await import("@tauri-apps/plugin-dialog");
  const picked = await save({ defaultPath: name, filters: [{ name: "PNG", extensions: ["png"] }] });
  return typeof picked === "string" ? picked : null;
}

export async function pickDirectory(): Promise<string | null> {
  if (!isTauri()) return null;
  const { open } = await import("@tauri-apps/plugin-dialog");
  const picked = await open({ directory: true, multiple: false });
  return typeof picked === "string" ? picked : null;
}

/** Raw RGBA from a command. `null` means the byte length does not match the size. */
export async function fetchPixels(
  command: string,
  args: Record<string, unknown>,
  width: number,
  height: number,
  stage: string,
): Promise<Uint8ClampedArray<ArrayBuffer> | null> {
  if (!(width > 0) || !(height > 0)) return null;
  tracePage(`${stage}: pixels requested`);
  const bytes = await call<ArrayBuffer | Uint8Array>(command, args);
  tracePage(`${stage}: pixels received`);
  const raw = bytes instanceof Uint8Array ? bytes : new Uint8Array(bytes);
  if (raw.byteLength !== width * height * 4) return null;
  return imagePixels(raw);
}

function imagePixels(raw: Uint8Array): Uint8ClampedArray<ArrayBuffer> {
  // Share the command's buffer instead of copying the whole screen again.
  if (raw.buffer instanceof ArrayBuffer) {
    return new Uint8ClampedArray(raw.buffer, raw.byteOffset, raw.byteLength);
  }
  const copy = new Uint8ClampedArray(raw.byteLength);
  copy.set(raw);
  return copy;
}

export function paintPixels(canvas: HTMLCanvasElement, pixels: Uint8ClampedArray<ArrayBuffer>, width: number, height: number, opaque = true): boolean {
  canvas.width = width;
  canvas.height = height;
  const ctx = canvas.getContext("2d", { alpha: !opaque });
  if (!ctx) return false;
  ctx.putImageData(new ImageData(pixels, width, height), 0, 0);
  return true;
}

export async function loadFrozen(path: string, width: number, height: number): Promise<ShotImage> {
  if (!isTauri() || !(width > 0) || !(height > 0)) return loadImage(path);
  const pixels = await fetchPixels("preview_rgba", { path }, width, height, "f1");
  if (!pixels) return loadImage(path);
  const canvas = document.createElement("canvas");
  if (!paintPixels(canvas, pixels, width, height)) return loadImage(path);
  tracePage("f1: pixels painted");
  const shot = canvas as HTMLCanvasElement & ShotImage;
  shot.naturalWidth = width;
  shot.naturalHeight = height;
  shot.src = "";
  shot.pixels = pixels;
  return shot;
}

export function loadImage(path: string): Promise<HTMLImageElement> {
  return new Promise((resolve, reject) => {
    const image = new Image();
    // The blob URL stays valid for the lifetime of the document. Revoking it on
    // a timer made editors that outlived the timeout show a broken image. The
    // URL is released when the document unloads.
    image.onload = () => resolve(image);
    image.onerror = () => reject(new Error("image"));
    // macOS serves local captures through asset://. Drawing that URL taints the
    // canvas, so color sampling and OCR export fail with SecurityError. Read the
    // file bytes and use a same-origin blob instead.
    if (isTauri() && path && !path.startsWith("data:") && !path.startsWith("blob:") && !path.startsWith("http")) {
      call<ArrayBuffer | Uint8Array>("read_image_bytes", { path })
        .then((bytes) => {
          const copy = bytes instanceof Uint8Array ? new Uint8Array(bytes) : new Uint8Array(bytes);
          image.src = URL.createObjectURL(new Blob([copy], { type: "image/png" }));
        })
        .catch((error: unknown) => reject(error instanceof Error ? error : new Error(String(error))));
      return;
    }
    const src = imageSrc(path);
    if (src.startsWith("http")) image.crossOrigin = "anonymous";
    image.src = src;
  });
}

async function fileToBase64(path: string): Promise<string> {
  const response = await fetch(imageSrc(path));
  const bytes = new Uint8Array(await response.arrayBuffer());
  let binary = "";
  const chunk = 0x4000;
  for (let index = 0; index < bytes.length; index += chunk) {
    binary += String.fromCharCode(...bytes.subarray(index, index + chunk));
  }
  return btoa(binary);
}

function demoCapture(): FrozenCapture {
  const requested = new URLSearchParams(window.location.search).get("mode") ?? "edit";
  const mode = requested === "copy" ? "copy" : "edit";
  const preset = requested === "window" ? { x: 70, y: 50, w: 560, h: 380 } : null;
  const canvas = document.createElement("canvas");
  canvas.width = 1280;
  canvas.height = 800;
  const ctx = canvas.getContext("2d");
  if (!ctx) {
    return { path: "", width: 1280, height: 800, originX: 0, originY: 0, windows: [], mode, preset, screens: [], generation: 1 };
  }
  ctx.fillStyle = "#e7e1d6";
  ctx.fillRect(0, 0, 1280, 800);
  ctx.fillStyle = "#f7f4ee";
  ctx.fillRect(70, 50, 560, 380);
  ctx.fillStyle = "#243028";
  ctx.fillRect(680, 90, 500, 460);
  ctx.fillStyle = "#1c1915";
  ctx.font = "32px Segoe UI";
  ctx.fillText("Shotlet", 110, 140);
  ctx.font = "18px Segoe UI";
  ctx.fillText("拖一块区域，标上箭头，钉在屏幕上。", 110, 190);
  ctx.fillStyle = "#f4efe4";
  ctx.fillText("窗口 B", 720, 160);
  const path = canvas.toDataURL("image/png");
  return {
    path,
    width: 1280,
    height: 800,
    originX: 0,
    originY: 0,
    windows: [
      { title: "笔记", x: 70, y: 50, width: 560, height: 380, z: 1 },
      { title: "参考", x: 680, y: 90, width: 500, height: 460, z: 0 },
    ],
    mode,
    preset,
    screens: [{ x: 0, y: 0, width: 1280, height: 800, path, screenX: 0, screenY: 0 }],
    generation: 1,
  };
}

export function rememberEditor(payload: EditorPayload) {
  sessionStorage.setItem("shotlet-editor", JSON.stringify(payload));
}
