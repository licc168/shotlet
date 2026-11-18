import { LogicalPosition, LogicalSize } from "@tauri-apps/api/dpi";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { useEffect, useRef, useState } from "react";
import { copyFile, imageSrc, isTauri, pinPath } from "../api";
import { useI18n } from "../i18n";
import {
  PIN_MAX_OPACITY,
  PIN_MIN_OPACITY,
  PIN_OPACITY_STEP,
  centeredOrigin,
  isDoubleClick,
  nextScale,
  pinWindowSize,
  type ClickStamp,
} from "../lib/pin";

export function Pin() {
  const { t } = useI18n();
  const [src, setSrc] = useState("");
  const [scale, setScale] = useState(1);
  const [opacity, setOpacity] = useState(1);
  const [menu, setMenu] = useState(false);
  const pinRef = useRef<HTMLDivElement>(null);
  const naturalRef = useRef<{ w: number; h: number } | null>(null);
  const lastClickRef = useRef<ClickStamp | null>(null);
  const resizeRequestRef = useRef(0);

  useEffect(() => {
    const id = new URLSearchParams(location.search).get("id") ?? "";
    pinPath(id).then(setSrc).catch(() => setSrc(""));
  }, []);

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
    const target = pinWindowSize(natural.w, natural.h, next);
    const request = ++resizeRequestRef.current;
    try {
      const win = getCurrentWindow();
      const factor = await win.scaleFactor();
      if (request !== resizeRequestRef.current) return;
      const size = await win.innerSize();
      const pos = await win.outerPosition();
      if (request !== resizeRequestRef.current) return;
      const logicalSize = size.toLogical(factor);
      const logicalPos = pos.toLogical(factor);
      const origin = centeredOrigin(logicalPos.x, logicalPos.y, logicalSize.width, logicalSize.height, target.w, target.h);
      await win.setSize(new LogicalSize(target.w, target.h));
      if (request !== resizeRequestRef.current) return;
      await win.setPosition(new LogicalPosition(origin.x, origin.y));
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

  if (!src) return <main className="pin empty">{t("busy")}</main>;

  return (
    <div
      ref={pinRef}
      className="pin"
      style={{ opacity }}
      onPointerDown={onPointerDown}
      onDoubleClick={() => void closePin()}
      onContextMenu={(event) => { event.preventDefault(); setMenu(true); }}
    >
      <img
        src={imageSrc(src)}
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
          <button onClick={() => void copyFile(src)}>{t("copy")}</button>
          <button onClick={() => void closePin()}>{t("cancel")}</button>
        </div>
      )}
      <p>{t("hintPin")}</p>
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
