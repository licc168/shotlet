import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { Annotator } from "../editor/Annotator";
import { aimOverlay, cancelCapture, commitHistory, copyAnnotated, copyCaptureRegion, copyPngBase64, currentCapture, isTauri, loadFrozen, ocrPngBase64, onEvent, openRecording, overlayReady, pinAnnotated, pinAnnotatedRgba, pinCaptureRegion, pinHistory, pickSavePath, scrollCapture, setOverlayFollow, writePngBase64, type ShotImage } from "../api";
import { screenCssSize } from "../editor/draw";
import { useI18n } from "../i18n";
import { clampRect, contains, isDragGlitch, normalize, square, type Rect } from "../lib/selection";
import type { FrozenCapture, ScreenImage, WindowInfo } from "../types";

function overlaySlot(): number {
  const raw = new URLSearchParams(window.location.search).get("slot");
  const slot = raw ? Number(raw) : 0;
  return Number.isFinite(slot) && slot >= 0 ? Math.floor(slot) : 0;
}

function screenOf(capture: FrozenCapture, slot: number): ScreenImage | null {
  const screens = capture.screens ?? [];
  if (screens.length === 0) {
    if (slot !== 0) return null;
    return { x: 0, y: 0, width: capture.width, height: capture.height, path: capture.path, screenX: capture.originX, screenY: capture.originY };
  }
  const found = screens[slot];
  return found?.path ? found : null;
}

function isCursorScreen(capture: FrozenCapture, screen: ScreenImage): boolean {
  return screen.screenX === capture.originX && screen.screenY === capture.originY;
}

export function Overlay() {
  const { t } = useI18n();
  const [capture, setCapture] = useState<FrozenCapture | null>(null);
  const [image, setImage] = useState<ShotImage | null>(null);
  const [selection, setSelection] = useState<Rect | null>(null);
  const [hover, setHover] = useState(0);
  const [draft, setDraft] = useState<Rect | null>(null);
  const [cursor, setCursor] = useState<{ x: number; y: number; hex: string } | null>(null);
  const [error, setError] = useState("");
  const stageRef = useRef<HTMLDivElement>(null);
  const drag = useRef<{ x: number; y: number; moved: boolean; lx: number; ly: number } | null>(null);
  const stopDragListen = useRef<(() => void) | null>(null);
  const draftRef = useRef<Rect | null>(null);
  const copying = useRef(false);
  const scrolling = useRef(false);
  const loupeRef = useRef<HTMLCanvasElement>(null);
  const raf = useRef(0);
  const pendingPoint = useRef<{ x: number; y: number } | null>(null);
  const sampleRef = useRef<HTMLCanvasElement | null>(null);
  const ownsFollow = useRef(false);
  const aimAt = useRef(0);
  const endDrag = useRef<() => void>(() => {});
  const [armed, setArmed] = useState(false);
  const slot = overlaySlot();

  function updateDraft(next: Rect | null) {
    draftRef.current = next;
    setDraft(next);
  }

  useEffect(() => {
    let dead = false;
    let seen = -1;
    let wave = 0;
    const stops: Array<() => void> = [];
    function dropFrame() {
      wave += 1;
      seen = -1;
      ownsFollow.current = false;
      drag.current = null;
      draftRef.current = null;
      setDraft(null);
      setCursor(null);
      setCapture(null);
      setImage(null);
      setSelection(null);
      setArmed(false);
    }
    async function apply(frozen: FrozenCapture | null, preloaded?: ShotImage | null) {
      if (dead || !frozen) return;
      const generation = frozen.generation ?? 0;
      const token = wave;
      // The cursor screen is shown first. This page also hears that event, and the
      // other displays arrive later with the same generation. Marking it seen before
      // this slot has pixels drops that later show, so the other screen stays blank.
      if (generation < seen) return;
      const screen = screenOf(frozen, slot);
      if (!screen) {
        if (generation === seen) return;
        ownsFollow.current = false;
        setCapture(null);
        setImage(null);
        setSelection(null);
        setArmed(false);
        return;
      }
      if (generation === seen) return;
      seen = generation;
      drag.current = null;
      draftRef.current = null;
      setDraft(null);
      setCursor(null);
      const cursorScreen = isCursorScreen(frozen, screen);
      ownsFollow.current = cursorScreen;
      setArmed(cursorScreen);
      try {
        const loaded = preloaded ?? await loadFrozen(screen.path || frozen.path, screen.width || frozen.width, screen.height || frozen.height);
        if (dead || token !== wave || seen !== generation) return;
        setImage(loaded);
        setError("");
      } catch (reason: unknown) {
        if (dead || token !== wave || seen !== generation) return;
        setImage(null);
        setError(reason instanceof Error ? reason.message : String(reason));
      }
      if (dead || token !== wave || seen !== generation) return;
      const preset = frozen.preset;
      let next: Rect | null = null;
      if (preset && cursorScreen) {
        next = { x: Math.max(0, preset.x), y: Math.max(0, preset.y), w: preset.w, h: preset.h };
      }
      setCapture(frozen);
      setSelection(next);
    }
    onEvent<{ armed?: boolean; slot?: number; path?: string; width?: number; height?: number; generation?: number }>("overlay-show", (payload) => {
      if (typeof payload?.slot === "number" && payload.slot !== slot) return;
      if (typeof payload?.armed === "boolean") setArmed(payload.armed);
      const pixels = payload?.path && payload.width && payload.height
        ? loadFrozen(payload.path, payload.width, payload.height).catch(() => null)
        : Promise.resolve(null);
      void Promise.all([currentCapture(), pixels]).then(([frozen, loaded]) => {
        if (payload?.generation != null && (frozen?.generation ?? 0) !== payload.generation) {
          void currentCapture().then((next) => {
            if ((next?.generation ?? 0) === payload.generation) void apply(next, loaded);
          });
          return;
        }
        void apply(frozen, loaded);
      });
    }).then((stop) => {
      if (dead) stop();
      else stops.push(stop);
    }).catch(() => undefined);
    onEvent("overlay-hide", () => {
      if (!dead) dropFrame();
    }).then((stop) => {
      if (dead) stop();
      else stops.push(stop);
    }).catch(() => undefined);
    onEvent<boolean | { slot?: number; armed?: boolean }>("overlay-arm", (payload) => {
      const next = typeof payload === "boolean" ? payload : payload?.armed;
      const eventSlot = typeof payload === "object" && payload ? payload.slot : undefined;
      if (typeof eventSlot === "number" && eventSlot !== slot) return;
      if (typeof next !== "boolean") return;
      setArmed(next);
      if (!next) {
        setCursor(null);
        endDrag.current();
      }
    }).then((stop) => {
      if (dead) stop();
      else stops.push(stop);
    }).catch(() => undefined);
    currentCapture().then(apply).catch((reason: unknown) => {
      if (!dead) setError(reason instanceof Error ? reason.message : String(reason));
    });
    return () => {
      dead = true;
      stops.forEach((stop) => stop());
      cancelAnimationFrame(raf.current);
      stopDragListen.current?.();
    };
  }, []);

  useEffect(() => {
    function onKey(event: KeyboardEvent) {
      if (event.key !== "Escape" || event.repeat) return;
      const target = event.target as HTMLElement | null;
      if (target && (target.tagName === "TEXTAREA" || target.tagName === "INPUT")) return;
      event.preventDefault();
      void cancelCapture();
    }
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  const picking = selection == null;
  useEffect(() => {
    if (!capture) return;
    if (ownsFollow.current) void setOverlayFollow(picking);
    else if (!picking) void setOverlayFollow(false);
  }, [capture, picking]);

  useEffect(() => {
    if (!capture || (!image && !error)) return;
    const screen = screenOf(capture, slot);
    void overlayReady(capture.generation ?? 0, screen ? isCursorScreen(capture, screen) : false);
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
    const screen = screenOf(capture, slot);
    if (!screen) return [];
    const source = screen.windows != null
      ? screen.windows
      : capture.windows.map((item) => ({ ...item, x: item.x - screen.x, y: item.y - screen.y }));
    return source
      .filter((item) => x >= item.x && y >= item.y && x <= item.x + item.width && y <= item.y + item.height)
      .sort((a, b) => a.z - b.z);
  }

  function windowUnder(x: number, y: number): WindowInfo | null {
    const windows = windowsAt(x, y);
    if (!windows.length) return null;
    const index = ((hover % windows.length) + windows.length) % windows.length;
    return windows[index] ?? null;
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
    return <div className={armed ? "overlay-root armed" : "overlay-root"}>{error && <p className="overlay-hint">{error}</p>}</div>;
  }

  const found = screenOf(capture, slot);
  if (!found) {
    return <div className="overlay-root" />;
  }
  const screen = found;
  const active = cursor ? windowUnder(cursor.x, cursor.y) : null;
  const live = draft ?? (active ? { x: active.x, y: active.y, w: active.width, h: active.height } : null);
  const shot = image;

  async function finishPin(region: Rect): Promise<boolean> {
    if (!isTauri()) return false;
    const x = Math.max(0, Math.round(region.x));
    const y = Math.max(0, Math.round(region.y));
    const w = Math.max(1, Math.round(Math.min(region.w, shot.naturalWidth - x)));
    const h = Math.max(1, Math.round(Math.min(region.h, shot.naturalHeight - y)));
    try {
      await pinCaptureRegion(
        screen.x + x,
        screen.y + y,
        w,
        h,
        screen.screenX,
        screen.screenY,
        x,
        y,
        shot.naturalWidth,
        shot.naturalHeight,
        screen.pointWidth || shot.naturalWidth,
        screen.pointHeight || shot.naturalHeight,
      );
      await cancelCapture();
      return true;
    } catch (reason: unknown) {
      setError(reason instanceof Error ? reason.message : String(reason));
      return false;
    }
  }

  async function finishCopy(region: Rect): Promise<boolean> {
    if (copying.current) return false;
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
      return true;
    } catch (reason: unknown) {
      copying.current = false;
      setError(reason instanceof Error ? reason.message : String(reason));
      return false;
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
    const start = { x: drag.current.x, y: drag.current.y };
    drag.current = null;
    if (moved && region && region.w > 4 && region.h > 4) {
      const next = clampRect(region, { x: 0, y: 0, w: shot.naturalWidth, h: shot.naturalHeight });
      if (capture?.mode === "copy") void finishCopy(next);
      else setSelection(next);
      return;
    }
    updateDraft(null);
    const target = windowUnder(start.x, start.y);
    if (!target) {
      void setOverlayFollow(true);
      return;
    }
    const next = { x: target.x, y: target.y, w: target.width, h: target.height };
    if (capture?.mode === "copy") void finishCopy(next);
    else setSelection(next);
  }
  endDrag.current = commitDrag;

  function trackDrag(start: { x: number; y: number }) {
    void setOverlayFollow(false);
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

  function aimHere() {
    setArmed(true);
    const now = Date.now();
    if (now - aimAt.current < 80) return;
    aimAt.current = now;
    void aimOverlay(screen.screenX, screen.screenY);
  }

  return (
    <div
      className={["overlay-root", armed ? "armed" : "", selection || draft ? "framed" : ""].filter(Boolean).join(" ")}
      onPointerEnter={() => {
        if (drag.current || selection) return;
        aimHere();
      }}
      onPointerLeave={(event) => {
        // A synthesized mouseleave fires when focus hops to the other display,
        // often while the pointer is still inside this one. Do not dim it here;
        // the screen under the cursor is armed by aimOverlay / overlay-arm.
        const bounds = event.currentTarget.getBoundingClientRect();
        const inside =
          event.clientX >= bounds.left &&
          event.clientX <= bounds.right &&
          event.clientY >= bounds.top &&
          event.clientY <= bounds.bottom;
        if (inside) return;
        setCursor(null);
        commitDrag();
      }}
    >
      <FrozenShot image={image} />
      {!armed && <div className="screen-dim" />}
      {selection ? (
        <Annotator
          image={image}
          selection={selection}
          onSelectionChange={setSelection}
          editableSelection
          mode="capture"
          desktop
          onClose={() => { void cancelCapture(); }}
          onScroll={() => {
            if (scrolling.current) return;
            scrolling.current = true;
            void scrollCapture({
              screenX: screen.screenX,
              screenY: screen.screenY,
              screenW: screen.pointWidth || image.naturalWidth,
              screenH: screen.pointHeight || image.naturalHeight,
              x: Math.round(selection.x),
              y: Math.round(selection.y),
              width: Math.round(selection.w),
              height: Math.round(selection.h),
              imageW: image.naturalWidth,
              imageH: image.naturalHeight,
              pointW: screen.pointWidth || image.naturalWidth,
              pointH: screen.pointHeight || image.naturalHeight,
            })
              .catch((reason: unknown) => {
                const message = reason instanceof Error ? reason.message : String(reason);
                if (message.includes("已取消")) return;
                setError(message);
              })
              .finally(() => {
                scrolling.current = false;
              });
          }}
          onPlainCopy={(region) => finishCopy(region)}
          onPlainPin={(region) => finishPin(region)}
          onAnnotatedPin={async (pixels, width, height, region, layers) => {
            const x = Math.max(0, Math.round(region.x));
            const y = Math.max(0, Math.round(region.y));
            const w = Math.max(1, Math.round(region.w));
            const h = Math.max(1, Math.round(region.h));
            try {
              const id = await pinAnnotatedRgba(pixels, width, height, {
                x: screen.x + x,
                y: screen.y + y,
                width: w,
                height: h,
                screenX: screen.screenX,
                screenY: screen.screenY,
                localX: x,
                localY: y,
                bitmapW: image.naturalWidth,
                bitmapH: image.naturalHeight,
                pointW: screen.pointWidth || image.naturalWidth,
                pointH: screen.pointHeight || image.naturalHeight,
              });
              if (id) void pinHistory(id, JSON.stringify(layers));
              await cancelCapture();
              return true;
            } catch {
              return false;
            }
          }}
          onRecord={() => {
            void openRecording(
              screen.screenX,
              screen.screenY,
              selection.x,
              selection.y,
              selection.w,
              selection.h,
              image.naturalWidth,
              image.naturalHeight,
              screen.pointWidth || image.naturalWidth,
              screen.pointHeight || image.naturalHeight,
            ).catch((reason: unknown) => {
              setError(reason instanceof Error ? reason.message : String(reason));
            });
          }}
          onAction={async (action, png, base, layers) => {
            if (action === "copy" && isTauri()) {
              await copyAnnotated(png, base, JSON.stringify(layers));
              await cancelCapture();
              return "";
            }
            let ocrText = "";
            if (action === "ocr") ocrText = await ocrPngBase64(png);
            if (action === "pin") {
              await pinAnnotated(
                png,
                base,
                JSON.stringify(layers),
                screen.screenX,
                screen.screenY,
                selection.x,
                selection.y,
                selection.w,
                selection.h,
                image.naturalWidth,
                image.naturalHeight,
                screen.pointWidth || image.naturalWidth,
                screen.pointHeight || image.naturalHeight,
              );
              await cancelCapture();
              return "";
            }
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
            if (action !== "ocr") await cancelCapture();
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
          aimHere();
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
          if (!armed) aimHere();
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
        onWheel={(event) => {
          if (!drag.current) setHover((value) => value + (event.deltaY > 0 ? 1 : -1));
        }}
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
      {error
        ? <p className="overlay-hint error">{error}</p>
        : !selection && armed && <p className="overlay-hint">{capture.mode === "copy" ? t("hintCopy") : t("hintPick")}</p>}
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

function FrozenShot({ image }: { image: ShotImage }) {
  const host = useRef<HTMLDivElement>(null);
  const size = screenCssSize(image.naturalWidth, image.naturalHeight, window.devicePixelRatio);
  useLayoutEffect(() => {
    const node = host.current;
    if (!node || !(image instanceof HTMLCanvasElement)) return;
    image.className = "frozen-screen";
    image.style.width = `${size.width}px`;
    image.style.height = `${size.height}px`;
    if (image.parentElement !== node) node.replaceChildren(image);
  }, [image, size.width, size.height]);
  if (image instanceof HTMLCanvasElement) return <div className="frozen-host" ref={host} />;
  return <img className="frozen-screen" src={image.src} alt="" draggable={false} style={{ width: size.width, height: size.height }} />;
}

function boxStyle(rect: Rect, image: ShotImage, stage: HTMLDivElement | null) {
  if (!stage) return { left: rect.x, top: rect.y, width: rect.w, height: rect.h };
  const bounds = stage.getBoundingClientRect();
  const sx = bounds.width / image.naturalWidth;
  const sy = bounds.height / image.naturalHeight;
  return { left: rect.x * sx, top: rect.y * sy, width: rect.w * sx, height: rect.h * sy };
}

function eventLeft(point: { x: number }, image: ShotImage, stage: HTMLDivElement | null) {
  if (!stage) return point.x;
  return (point.x / image.naturalWidth) * stage.getBoundingClientRect().width + 16;
}

function eventTop(point: { y: number }, image: ShotImage, stage: HTMLDivElement | null) {
  if (!stage) return point.y;
  return (point.y / image.naturalHeight) * stage.getBoundingClientRect().height + 16;
}

