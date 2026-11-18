import { PhysicalPosition, PhysicalSize } from "@tauri-apps/api/dpi";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { useEffect, useRef, useState, type CSSProperties } from "react";
import { copyFile, isTauri, loadImage, pinPath } from "../api";
import { useI18n } from "../i18n";
import {
  PIN_MAX_OPACITY,
  PIN_MIN_OPACITY,
  PIN_OPACITY_STEP,
  centeredOrigin,
  isDoubleClick,
  nextScale,
  pinFrameOrigin,
  pinFrameSize,
  pinGlowCssPx,
  pinWindowSize,
  type ClickStamp,
} from "../lib/pin";

export function Pin() {
  const { t } = useI18n();
  const [src, setSrc] = useState("");
  const [file, setFile] = useState("");
  const [failed, setFailed] = useState("");
  const [scale, setScale] = useState(1);
  const [opacity, setOpacity] = useState(1);
  const [menu, setMenu] = useState(false);
  const pinRef = useRef<HTMLDivElement>(null);
  const naturalRef = useRef<{ w: number; h: number } | null>(null);
  const lastClickRef = useRef<ClickStamp | null>(null);
  const resizeRequestRef = useRef(0);
  const placedRef = useRef(false);

  useEffect(() => {
    const id = new URLSearchParams(location.search).get("id") ?? "";
    let cancel = false;
    pinPath(id)
      .then(async (path) => {
        if (cancel) return;
        if (!path) {
          setFailed(t("empty"));
          return;
        }
        setFile(path);
        if (path.startsWith("data:") || path.startsWith("blob:") || path.startsWith("http")) {
          setSrc(path);
          return;
        }
        // asset:// taints or fails to paint inside a transparent pin window.
        // Same-origin bytes are what the capture overlay already uses.
        const image = await loadImage(path);
        if (!cancel) setSrc(image.src);
      })
      .catch((reason: unknown) => {
        if (!cancel) setFailed(reason instanceof Error ? reason.message : String(reason));
      });
    return () => {
      cancel = true;
    };
  }, [t]);

  useEffect(() => {
    const node = pinRef.current;
    if (!node) return;
    const onWheel = (event: WheelEvent) => {
      event.preventDefault();
      if (event.ctrlKey || event.metaKey) {
        setOpacity((value) => Math.min(PIN_MAX_OPACITY, Math.max(PIN_MIN_OPACITY, value + (event.deltaY < 0 ? PIN_OPACITY_STEP : -PIN_OPACITY_STEP))));
      } else {
        setScale((value) => nextScale(value, event.deltaY < 0));
      }
    };
    node.addEventListener("wheel", onWheel, { passive: false });
    return () => node.removeEventListener("wheel", onWheel);
  }, [src]);

  useEffect(() => {
    void fitPinWindow(scale);
  }, [scale]);

  async function fitPinWindow(next: number) {
    const natural = naturalRef.current;
    if (!natural || !isTauri()) return;
    const image = pinWindowSize(natural.w, natural.h, next);
    // The window is larger than the picture so the edge light can sit outside the cut.
    const frame = pinFrameSize(image.w, image.h);
    const request = ++resizeRequestRef.current;
    try {
      const win = getCurrentWindow();
      if (request !== resizeRequestRef.current) return;
      // First layout is the screen cut: same pixel size, same top-left.
      // Later zoom grows around the center. The glow margin stays fixed.
      if (!placedRef.current) {
        placedRef.current = true;
        const params = new URLSearchParams(location.search);
        const x = Number(params.get("x"));
        const y = Number(params.get("y"));
        await win.setSize(new PhysicalSize(frame.w, frame.h));
        if (request !== resizeRequestRef.current) return;
        if (params.has("x") && params.has("y") && Number.isFinite(x) && Number.isFinite(y)) {
          const origin = pinFrameOrigin(Math.round(x), Math.round(y));
          await win.setPosition(new PhysicalPosition(origin.x, origin.y));
        }
        return;
      }
      const size = await win.innerSize();
      const pos = await win.outerPosition();
      if (request !== resizeRequestRef.current) return;
      const origin = centeredOrigin(pos.x, pos.y, size.width, size.height, frame.w, frame.h);
      await win.setSize(new PhysicalSize(frame.w, frame.h));
      if (request !== resizeRequestRef.current) return;
      await win.setPosition(new PhysicalPosition(Math.round(origin.x), Math.round(origin.y)));
    } catch {
      // The pin window may already have been closed.
    }
  }

  function onImageLoad(event: React.SyntheticEvent<HTMLImageElement>) {
    const image = event.currentTarget;
    if (!image.naturalWidth) return;
    naturalRef.current = { w: image.naturalWidth, h: image.naturalHeight };
    void fitPinWindow(scale);
  }

  function onPointerDown(event: React.PointerEvent) {
    if (event.button !== 0) return;
    if ((event.target as HTMLElement).closest("button, .pin-menu")) return;
    const stamp = { time: Date.now(), x: event.clientX, y: event.clientY };
    if (event.detail >= 2 || isDoubleClick(lastClickRef.current, stamp)) {
      lastClickRef.current = null;
      event.preventDefault();
      void closePin();
      return;
    }
    lastClickRef.current = stamp;
    if (!isTauri()) return;
    event.preventDefault();
    // Must start in this pointerdown turn. Awaiting the import first lets Windows
    // enter a modal move loop after the button is already up, which captures the mouse.
    void getCurrentWindow().startDragging();
  }

  if (!src) return <main className={failed ? "pin empty failed" : "pin empty"}>{failed}</main>;

  const glow = `${pinGlowCssPx(window.devicePixelRatio)}px`;
  return (
    <div
      ref={pinRef}
      className="pin"
      style={{ opacity, "--pin-glow": glow } as CSSProperties}
      onPointerDown={onPointerDown}
      onDoubleClick={() => void closePin()}
      onContextMenu={(event) => { event.preventDefault(); setMenu(true); }}
    >
      <div className="pin-glow" aria-hidden="true" />
      <img
        src={src}
        alt=""
        draggable={false}
        onLoad={onImageLoad}
        ref={(image) => {
          if (image?.complete && image.naturalWidth && !naturalRef.current) {
            onImageLoad({ currentTarget: image } as React.SyntheticEvent<HTMLImageElement>);
          }
        }}
      />
      {menu && (
        <div className="pin-menu">
          <button onClick={() => void copyFile(file || src)}>{t("copy")}</button>
          <button onClick={() => void closePin()}>{t("cancel")}</button>
        </div>
      )}
    </div>
  );
}

async function closePin() {
  if (!isTauri()) {
    window.close();
    return;
  }
  await getCurrentWindow().close();
}
