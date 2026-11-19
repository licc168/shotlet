import { convertFileSrc } from "@tauri-apps/api/core";
import { defaultSettings, type CommitBody, type EditorPayload, type FrozenCapture, type HistoryItem, type Settings } from "./types";

/** A decoded screenshot. An image element or a canvas that already holds the captured pixels. */
export type ShotImage = CanvasImageSource & {
  naturalWidth: number;
  naturalHeight: number;
  src: string;
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
  const { invoke } = await import("@tauri-apps/api/core");
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

export async function overlayReady(generation: number): Promise<void> {
  if (!isTauri()) return;
  await call("overlay_ready", { generation });
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
  if (!isTauri()) {
    sessionStorage.setItem("shotlet-pin", `data:image/png;base64,${data}`);
    window.open("/?view=pin&id=browser", "shotlet-pin", "popup,width=480,height=360");
    return;
  }
  await call("pin_png_base64", { data, x, y });
}

export async function pinFile(path: string, x: number, y: number): Promise<void> {
  if (!isTauri()) return pinPngBase64(await fileToBase64(path), x, y);
  await call("pin_file", { path, x, y });
}

export async function pinPath(id: string): Promise<string> {
  if (!isTauri()) return sessionStorage.getItem("shotlet-pin") ?? "";
  return call<string>("pin_path", { id });
}

export async function ocrPngBase64(data: string): Promise<string> {
  if (!isTauri()) return "Shotlet";
  return call<string>("ocr_png_base64", { data });
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

export async function openRecording(x: number, y: number, width: number, height: number): Promise<void> {
  if (!isTauri()) {
    window.alert("区域录制需要桌面端。");
    return;
  }
  await call("open_recording", { x, y, width, height });
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

export async function loadFrozen(path: string, width: number, height: number): Promise<ShotImage> {
  if (!isTauri() || !(width > 0) || !(height > 0)) return loadImage(path);
  const bytes = await call<ArrayBuffer | Uint8Array>("preview_rgba", { path });
  const raw = bytes instanceof Uint8Array ? bytes : new Uint8Array(bytes);
  if (raw.byteLength !== width * height * 4) return loadImage(path);
  const canvas = document.createElement("canvas");
  canvas.width = width;
  canvas.height = height;
  const ctx = canvas.getContext("2d");
  if (!ctx) return loadImage(path);
  const pixels = new Uint8ClampedArray(raw.byteLength);
  pixels.set(raw);
  ctx.putImageData(new ImageData(pixels, width, height), 0, 0);
  const shot = canvas as HTMLCanvasElement & ShotImage;
  shot.naturalWidth = width;
  shot.naturalHeight = height;
  shot.src = "";
  return shot;
}

export function loadImage(path: string): Promise<HTMLImageElement> {
  return new Promise((resolve, reject) => {
    const image = new Image();
    image.onload = () => {
      const src = image.src;
      resolve(image);
      if (src.startsWith("blob:")) setTimeout(() => URL.revokeObjectURL(src), 60_000);
    };
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
