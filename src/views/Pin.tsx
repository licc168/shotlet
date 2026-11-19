import { getCurrentWindow } from "@tauri-apps/api/window";
import { useEffect, useRef, useState, type CSSProperties } from "react";
import { copyPin, fetchPixels, isTauri, loadImage, onEvent, paintPixels, pinPath, pinReady, pinStandby, pinZoom, tracePage } from "../api";
import { useI18n } from "../i18n";
import {
  PIN_MAX_OPACITY,
  PIN_MIN_OPACITY,
  PIN_OPACITY_STEP,
  isDoubleClick,
  nextScale,
  pinGlowCssPx,
  type ClickStamp,
} from "../lib/pin";

type PinAssign = { id: string; width: number; height: number };

export function Pin() {
  const { t } = useI18n();
  const tRef = useRef(t);
  tRef.current = t;
  const [failed, setFailed] = useState("");
  const [painted, setPainted] = useState(false);
  const [opacity, setOpacity] = useState(1);
  const [menu, setMenu] = useState(false);
  const [glow, setGlow] = useState(() => pinGlowCssPx(window.devicePixelRatio));
  const pinRef = useRef<HTMLDivElement>(null);
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const idRef = useRef("");
  const lastClickRef = useRef<ClickStamp | null>(null);
  const revealedRef = useRef(false);
  const scaleRef = useRef(1);

  useEffect(() => {
    const onResize = () => setGlow(pinGlowCssPx(window.devicePixelRatio));
    window.addEventListener("resize", onResize);
    return () => window.removeEventListener("resize", onResize);
  }, []);

  useEffect(() => {
    let cancel = false;
    const params = new URLSearchParams(location.search);
    const initialId = params.get("id") ?? "";
    const initialW = Number(params.get("w") || 0);
    const initialH = Number(params.get("h") || 0);
    async function show(id: string, width: number, height: number) {
      const canvas = canvasRef.current;
      if (cancel || !canvas || !id) return;
      idRef.current = id;
      setFailed("");
      try {
        if (!isTauri()) {
          const path = await pinPath(id);
          if (cancel) return;
          if (!path) {
            setFailed(tRef.current("empty"));
            return;
          }
          const image = await loadImage(path);
          if (cancel) return;
          canvas.width = image.naturalWidth;
          canvas.height = image.naturalHeight;
          canvas.getContext("2d")?.drawImage(image, 0, 0);
          setPainted(true);
          return;
        }
        const pixels = await fetchPixels("pin_pixels", { id }, width, height, "pin");
        if (cancel) return;
        if (!pixels || !paintPixels(canvas, pixels, width, height, false)) {
          setFailed(tRef.current("empty"));
          return;
        }
        tracePage("pin: pixels painted");
        setPainted(true);
        void revealPin(revealedRef);
      } catch (reason: unknown) {
        if (!cancel) setFailed(reason instanceof Error ? reason.message : String(reason));
      }
    }
    let stop: () => void = () => undefined;
    if (initialId && (initialW > 0 && initialH > 0 || !isTauri())) {
      void show(initialId, initialW, initialH);
    } else if (isTauri()) {
      void pinStandby();
      onEvent<PinAssign>("pin-assign", (payload) => {
        if (cancel || !payload?.id || !(payload.width > 0) || !(payload.height > 0)) return;
        tracePage("pin: assign received");
        revealedRef.current = false;
        scaleRef.current = 1;
        void show(payload.id, payload.width, payload.height);
      }).then((unlisten) => {
        if (cancel) unlisten();
        else stop = unlisten;
      }).catch(() => undefined);
    }
    return () => {
      cancel = true;
      stop();
    };
  }, []);

  useEffect(() => {
    const node = pinRef.current;
    if (!node) return;
    const onWheel = (event: WheelEvent) => {
      if (!painted) return;
      event.preventDefault();
      if (event.ctrlKey || event.metaKey) {
        setOpacity((value) => Math.min(PIN_MAX_OPACITY, Math.max(PIN_MIN_OPACITY, value + (event.deltaY < 0 ? PIN_OPACITY_STEP : -PIN_OPACITY_STEP))));
      } else if (isTauri()) {
        scaleRef.current = nextScale(scaleRef.current, event.deltaY < 0);
        void pinZoom(scaleRef.current);
      }
    };
    node.addEventListener("wheel", onWheel, { passive: false });
    return () => node.removeEventListener("wheel", onWheel);
  }, [painted]);

  useEffect(() => {
    if (failed) void revealPin(revealedRef);
  }, [failed]);

  function onPointerDown(event: React.PointerEvent) {
    if (event.button !== 0 || !painted) return;
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

  return (
    <div
      ref={pinRef}
      className={failed ? "pin empty failed" : "pin"}
      style={{ opacity, "--pin-glow": `${glow}px` } as CSSProperties}
      onPointerDown={onPointerDown}
      onDoubleClick={() => { if (painted) void closePin(); }}
      onContextMenu={(event) => { event.preventDefault(); if (painted) setMenu(true); }}
    >
      {failed ? failed : <div className="pin-glow" aria-hidden="true" />}
      <canvas ref={canvasRef} />
      {menu && (
        <div className="pin-menu">
          <button onClick={() => void copyPin(idRef.current)}>{t("copy")}</button>
          <button onClick={() => void closePin()}>{t("cancel")}</button>
        </div>
      )}
    </div>
  );
}

async function revealPin(revealed: { current: boolean }) {
  if (!isTauri() || revealed.current) return;
  revealed.current = true;
  // The decode callback runs before the image is on the compositor. Wait one
  // paint, but don't wait forever if a cloaked window throttles frames.
  await new Promise<void>((resolve) => {
    let settled = false;
    const finish = () => {
      if (settled) return;
      settled = true;
      resolve();
    };
    requestAnimationFrame(() => requestAnimationFrame(finish));
    window.setTimeout(finish, 48);
  });
  try {
    await pinReady();
  } catch {
    // The pin window may already have been closed.
  }
}

async function closePin() {
  if (!isTauri()) {
    window.close();
    return;
  }
  await getCurrentWindow().close();
}
