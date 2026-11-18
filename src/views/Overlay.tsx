import { useEffect, useRef, useState } from "react";
import { Annotator } from "../editor/Annotator";
import { cancelCapture, commitHistory, copyCaptureRegion, copyPngBase64, currentCapture, ffmpegAvailable, isTauri, loadImage, ocrPngBase64, onEvent, overlayReady, pinPngBase64, pickSavePath, scrollCapture, setOverlayFollow, startRecording, writePngBase64 } from "../api";
import { useI18n } from "../i18n";
import { clampRect, contains, isDragGlitch, normalize, square, type Rect } from "../lib/selection";
import type { FrozenCapture, ScreenImage, WindowInfo } from "../types";

function screenOf(capture: FrozenCapture): ScreenImage {
  const found = capture.screens?.find((item) => item.path) ?? capture.screens?.[0];
  if (found?.path) return found;
  return { x: 0, y: 0, width: capture.width, height: capture.height, path: capture.path, screenX: capture.originX, screenY: capture.originY };
}

export function Overlay() {
  const { t } = useI18n();
  const [capture, setCapture] = useState<FrozenCapture | null>(null);
  const [image, setImage] = useState<HTMLImageElement | null>(null);
  const [selection, setSelection] = useState<Rect | null>(null);
  const [hover, setHover] = useState(0);
  const [draft, setDraft] = useState<Rect | null>(null);
  const [cursor, setCursor] = useState<{ x: number; y: number; hex: string } | null>(null);
  const [ffmpeg, setFfmpeg] = useState(false);
  const [error, setError] = useState("");
  const stageRef = useRef<HTMLDivElement>(null);
  const drag = useRef<{ x: number; y: number; moved: boolean; lx: number; ly: number } | null>(null);
  const stopDragListen = useRef<(() => void) | null>(null);
  const draftRef = useRef<Rect | null>(null);
  const copying = useRef(false);
  const loupeRef = useRef<HTMLCanvasElement>(null);
  const raf = useRef(0);
  const pendingPoint = useRef<{ x: number; y: number } | null>(null);
  const sampleRef = useRef<HTMLCanvasElement | null>(null);

  function updateDraft(next: Rect | null) {
    draftRef.current = next;
    setDraft(next);
  }

  useEffect(() => {
    let dead = false;
    let seen = -1;
    let unlisten = () => {};
    async function apply(frozen: FrozenCapture | null) {
      if (dead || !frozen) return;
      const generation = frozen.generation ?? 0;
      if (generation === seen) return;
      seen = generation;
      const screen = screenOf(frozen);
      drag.current = null;
      draftRef.current = null;
      setDraft(null);
      try {
        const loaded = await loadImage(screen.path || frozen.path);
        if (dead || seen !== generation) return;
        setImage(loaded);
        setError("");
      } catch (reason: unknown) {
        if (dead || seen !== generation) return;
        setImage(null);
        setError(reason instanceof Error ? reason.message : String(reason));
      }
      const preset = frozen.preset;
      let next: Rect | null = null;
      if (preset) {
        const cx = preset.x + preset.w / 2;
        const cy = preset.y + preset.h / 2;
        const inside = cx >= screen.x && cy >= screen.y && cx < screen.x + screen.width && cy < screen.y + screen.height;
        if (inside) next = { x: Math.max(0, preset.x - screen.x), y: Math.max(0, preset.y - screen.y), w: preset.w, h: preset.h };
      }
      setCapture(frozen);
      setSelection(next);
    }
    onEvent("overlay-show", () => { void currentCapture().then(apply); }).then((stop) => {
      if (dead) stop();
      else unlisten = stop;
    }).catch(() => undefined);
    currentCapture().then(apply).catch((reason: unknown) => {
      if (!dead) setError(reason instanceof Error ? reason.message : String(reason));
    });
    if (!isTauri() && new URLSearchParams(window.location.search).get("ffmpeg") === "1") setFfmpeg(true);
    else ffmpegAvailable().then(setFfmpeg).catch(() => setFfmpeg(false));
    return () => {
      dead = true;
      unlisten();
      cancelAnimationFrame(raf.current);
      stopDragListen.current?.();
    };
  }, []);

  const picking = selection == null;
  useEffect(() => {
    if (!capture) return;
    void setOverlayFollow(picking);
  }, [capture, picking]);

  useEffect(() => {
    if (!capture || (!image && !error)) return;
    void overlayReady(capture.generation ?? 0);
  }, [capture, image, error]);

  function toImage(event: { clientX: number; clientY: number }) {
    const stage = stageRef.current;
    if (!stage || !image) return { x: 0, y: 0 };
    const rect = stage.getBoundingClientRect();
    return {
      x: ((event.clientX - rect.left) / rect.width) * image.naturalWidth,
      y: ((event.clientY - rect.top) / rect.height) * image.naturalHeight,
    };
  }

  function windowsAt(x: number, y: number): WindowInfo[] {
    if (!capture) return [];
    const screen = screenOf(capture);
    return capture.windows
      .map((item) => ({ ...item, x: item.x - screen.x, y: item.y - screen.y }))
      .filter((item) => x >= item.x && y >= item.y && x <= item.x + item.width && y <= item.y + item.height)
      .sort((a, b) => a.z - b.z);
  }

  function sampleHex(point: { x: number; y: number }) {
    if (!image) return "#000000";
    const canvas = sampleRef.current ?? document.createElement("canvas");
    sampleRef.current = canvas;
    canvas.width = 1;
    canvas.height = 1;
    const ctx = canvas.getContext("2d", { willReadFrequently: true });
    if (!ctx) return "#000000";
    ctx.drawImage(image, Math.round(point.x), Math.round(point.y), 1, 1, 0, 0, 1, 1);
    const pixel = ctx.getImageData(0, 0, 1, 1).data;
    return `#${[pixel[0], pixel[1], pixel[2]].map((value) => value.toString(16).padStart(2, "0")).join("")}`;
  }

  function paintLoupe(point: { x: number; y: number }) {
    setCursor({ ...point, hex: sampleHex(point) });
  }

  useEffect(() => {
    const loupe = loupeRef.current;
    if (!loupe || !image || !cursor) return;
    const ctx = loupe.getContext("2d");
    if (!ctx) return;
    ctx.imageSmoothingEnabled = false;
    ctx.clearRect(0, 0, 120, 120);
    ctx.drawImage(image, cursor.x - 8, cursor.y - 8, 16, 16, 0, 0, 120, 120);
    ctx.strokeStyle = "#3ddc97";
    ctx.strokeRect(56, 56, 8, 8);
  }, [cursor, image]);

  if (!capture || !image) {
    return <div className="overlay-root">{error && <p className="overlay-hint">{error}</p>}</div>;
  }

  const screen = screenOf(capture);
  const pointWindows = cursor ? windowsAt(cursor.x, cursor.y) : [];
  const active = pointWindows.length ? pointWindows[hover % pointWindows.length] : null;
  const live = draft ?? (active ? { x: active.x, y: active.y, w: active.width, h: active.height } : null);
  const shot = image;

  async function finishCopy(region: Rect) {
    if (copying.current) return;
    copying.current = true;
    const x = Math.max(0, Math.round(region.x));
    const y = Math.max(0, Math.round(region.y));
    const w = Math.max(1, Math.round(Math.min(region.w, shot.naturalWidth - x)));
    const h = Math.max(1, Math.round(Math.min(region.h, shot.naturalHeight - y)));
    try {
      if (isTauri()) {
        await copyCaptureRegion(screen.x + x, screen.y + y, w, h);
      } else {
        const canvas = document.createElement("canvas");
        canvas.width = w;
        canvas.height = h;
        const ctx = canvas.getContext("2d");
        if (!ctx) throw new Error("canvas");
        ctx.drawImage(shot, x, y, w, h, 0, 0, w, h);
        const png = canvas.toDataURL("image/png").split(",")[1] ?? "";
        await commitHistory({
          pngBase64: png,
          basePngBase64: png,
          document: "[]",
          ocrText: "",
          width: w,
          height: h,
          name: "自动复制",
        });
        try {
          await copyPngBase64(png);
        } catch {
          // The browser can refuse clipboard writes. The history item is already stored.
        }
      }
      await cancelCapture();
    } catch (reason: unknown) {
      copying.current = false;
      setError(reason instanceof Error ? reason.message : String(reason));
    }
  }

  function commitDrag() {
    if (!drag.current) return;
    stopDragListen.current?.();
    stopDragListen.current = null;
    cancelAnimationFrame(raf.current);
    raf.current = 0;
    const region = draftRef.current;
    const moved = drag.current.moved;
    drag.current = null;
    if (moved && region && region.w > 4 && region.h > 4) {
      const next = clampRect(region, { x: 0, y: 0, w: shot.naturalWidth, h: shot.naturalHeight });
      if (capture?.mode === "copy") void finishCopy(next);
      else setSelection(next);
    } else updateDraft(null);
  }

  function trackDrag(start: { x: number; y: number }) {
    const bounds = { w: shot.naturalWidth, h: shot.naturalHeight };
    const onMove = (event: PointerEvent) => {
      const current = drag.current;
      if (!current || (event.buttons & 1) === 0) return;
      const raw = toImage(event);
      if (isDragGlitch(raw, { x: current.lx, y: current.ly }, bounds)) return;
      const point = {
        x: Math.min(bounds.w, Math.max(0, raw.x)),
        y: Math.min(bounds.h, Math.max(0, raw.y)),
      };
      if (Math.hypot(point.x - current.x, point.y - current.y) > 3) current.moved = true;
      current.lx = point.x;
      current.ly = point.y;
      draftRef.current = clampRect(
        event.shiftKey ? square(current.x, current.y, point.x, point.y) : normalize(current.x, current.y, point.x, point.y),
        { x: 0, y: 0, ...bounds },
      );
      if (raf.current) return;
      raf.current = requestAnimationFrame(() => {
        raf.current = 0;
        if (drag.current) updateDraft(draftRef.current);
      });
    };
    const onUp = () => commitDrag();
    stopDragListen.current?.();
    window.addEventListener("pointermove", onMove);
    window.addEventListener("pointerup", onUp);
    window.addEventListener("pointercancel", onUp);
    stopDragListen.current = () => {
      window.removeEventListener("pointermove", onMove);
      window.removeEventListener("pointerup", onUp);
      window.removeEventListener("pointercancel", onUp);
    };
    drag.current = { x: start.x, y: start.y, lx: start.x, ly: start.y, moved: false };
  }

  return (
    <div className="overlay-root">
      <img className="frozen-screen" src={image.src} alt="" draggable={false} />
      {selection ? (
        <Annotator
          image={image}
          selection={selection}
          onSelectionChange={setSelection}
          editableSelection
          mode="capture"
          desktop
          ffmpeg={ffmpeg}
          onClose={() => { void cancelCapture(); }}
          onScroll={() => void scrollCapture(screen.screenX + selection.x, screen.screenY + selection.y, selection.w, selection.h)}
          onRecord={(choice) => {
            void startRecording(screen.screenX + selection.x, screen.screenY + selection.y, selection.w, selection.h, choice).catch((reason: unknown) => {
              setError(reason instanceof Error ? reason.message : String(reason));
            });
          }}
          onAction={async (action, png, base, layers) => {
            let ocrText = "";
            if (action === "ocr") ocrText = await ocrPngBase64(png);
            const pinAt = action === "pin"
              ? { x: screen.screenX + selection.x, y: screen.screenY + selection.y }
              : null;
            if (pinAt) await cancelCapture();
            const item = await commitHistory({
              pngBase64: png,
              basePngBase64: base,
              document: JSON.stringify(layers),
              ocrText,
              width: Math.round(selection.w),
              height: Math.round(selection.h),
              name: "",
            });
            if (action === "copy") await copyPngBase64(png);
            if (action === "save") {
              const dest = await pickSavePath(`${item.name}.png`);
              if (dest) await writePngBase64(png, dest);
            }
            if (pinAt) await pinPngBase64(png, pinAt.x, pinAt.y);
            if (action !== "ocr" && !pinAt) await cancelCapture();
            return ocrText;
          }}
        />
      ) : (
      <div
        className="stage"
        ref={stageRef}
        style={{ width: "100%", height: "100%" }}
        onPointerDown={(event) => {
          if (event.button !== 0) return;
          event.preventDefault();
          event.currentTarget.focus();
          const point = toImage(event);
          const start = {
            x: Math.min(shot.naturalWidth, Math.max(0, point.x)),
            y: Math.min(shot.naturalHeight, Math.max(0, point.y)),
          };
          trackDrag(start);
          updateDraft(normalize(start.x, start.y, start.x, start.y));
        }}
        onPointerMove={(event) => {
          if (drag.current) return;
          const point = toImage(event);
          pendingPoint.current = point;
          if (raf.current) return;
          raf.current = requestAnimationFrame(() => {
            raf.current = 0;
            if (!drag.current && pendingPoint.current) paintLoupe(pendingPoint.current);
          });
        }}
        onPointerUp={() => commitDrag()}
        onPointerCancel={() => commitDrag()}
        onDoubleClick={(event) => {
          const point = toImage(event);
          const windows = windowsAt(point.x, point.y);
          const target = windows[hover % Math.max(windows.length, 1)];
          if (target) setSelection({ x: target.x, y: target.y, w: target.width, h: target.height });
        }}
        onWheel={(event) => {
          if (!drag.current) setHover((value) => value + (event.deltaY > 0 ? 1 : -1));
        }}
        onKeyDown={(event) => { if (event.key === "Escape") void cancelCapture(); }}
        tabIndex={0}
      >
        {live && contains({ x: 0, y: 0, w: image.naturalWidth, h: image.naturalHeight }, live.x, live.y) && (
          <>
            <Shade box={boxStyle(live, image, stageRef.current)} />
            <div className="hole" style={boxStyle(live, image, stageRef.current)}>
              {active && !draft && <em>{active.title}</em>}
            </div>
          </>
        )}
        {cursor && !draft && (
          <div className="loupe" style={{ left: eventLeft(cursor, image, stageRef.current), top: eventTop(cursor, image, stageRef.current) }}>
            <canvas ref={loupeRef} width={120} height={120} />
            <span>{cursor.hex}</span>
          </div>
        )}
      </div>
      )}
      {!selection && <p className="overlay-hint">{capture.mode === "copy" ? t("hintCopy") : t("hintPick")}</p>}
    </div>
  );
}

function Shade({ box }: { box: { left: number; top: number; width: number; height: number } }) {
  if (box.width < 1 || box.height < 1) return null;
  return (
    <>
      <div className="shade top" style={{ height: Math.max(0, box.top) }} />
      <div className="shade left" style={{ top: box.top, width: Math.max(0, box.left), height: box.height }} />
      <div className="shade right" style={{ top: box.top, left: box.left + box.width, height: box.height }} />
      <div className="shade bottom" style={{ top: box.top + box.height }} />
    </>
  );
}

function boxStyle(rect: Rect, image: HTMLImageElement, stage: HTMLDivElement | null) {
  if (!stage) return { left: rect.x, top: rect.y, width: rect.w, height: rect.h };
  const bounds = stage.getBoundingClientRect();
  const sx = bounds.width / image.naturalWidth;
  const sy = bounds.height / image.naturalHeight;
  return { left: rect.x * sx, top: rect.y * sy, width: rect.w * sx, height: rect.h * sy };
}

function eventLeft(point: { x: number }, image: HTMLImageElement, stage: HTMLDivElement | null) {
  if (!stage) return point.x;
  return (point.x / image.naturalWidth) * stage.getBoundingClientRect().width + 16;
}

function eventTop(point: { y: number }, image: HTMLImageElement, stage: HTMLDivElement | null) {
  if (!stage) return point.y;
  return (point.y / image.naturalHeight) * stage.getBoundingClientRect().height + 16;
}

