import { getCurrentWindow } from "@tauri-apps/api/window";
import { useEffect, useRef, useState, type CSSProperties } from "react";
import { copyFile, isTauri, loadImage, onEvent, pinPath, pinReady, pinStandby, pinZoom, tracePage } from "../api";
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

export function Pin() {
  const { t } = useI18n();
  const tRef = useRef(t);
  tRef.current = t;
  const [src, setSrc] = useState("");
  const [file, setFile] = useState("");
  const [failed, setFailed] = useState("");
  const [opacity, setOpacity] = useState(1);
  const [menu, setMenu] = useState(false);
  const [glow, setGlow] = useState(() => pinGlowCssPx(window.devicePixelRatio));
  const pinRef = useRef<HTMLDivElement>(null);
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
    const id = params.get("id") ?? "";
    async function show(path: string) {
      if (cancel) return;
      if (!path) {
        setFailed(tRef.current("empty"));
        return;
      }
      setFailed("");
      setFile(path);
      if (path.startsWith("data:") || path.startsWith("blob:") || path.startsWith("http")) {
        setSrc(path);
        return;
      }
      // asset:// taints or fails to paint inside a transparent pin window.
      const image = await loadImage(path);
      tracePage("pin: pixels arrived");
      if (!cancel) setSrc(image.src);
    }
    let stop: () => void = () => undefined;
    if (id) {
      pinPath(id).then(show).catch((reason: unknown) => {
        if (!cancel) setFailed(reason instanceof Error ? reason.message : String(reason));
      });
    } else if (isTauri()) {
      // A warmed window has no picture yet. Report once, then wait for the picture.
      void pinStandby();
      onEvent<{ path: string }>("pin-assign", (payload) => {
        if (cancel || !payload?.path) return;
        tracePage("pin: assign received");
        revealedRef.current = false;
        void show(payload.path);
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
  }, [src]);

  useEffect(() => {
    if (failed) void revealPin(revealedRef);
  }, [failed]);

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

  return (
    <div
      ref={pinRef}
      className="pin"
      style={{ opacity, "--pin-glow": `${glow}px` } as CSSProperties}
      onPointerDown={onPointerDown}
      onDoubleClick={() => void closePin()}
      onContextMenu={(event) => { event.preventDefault(); setMenu(true); }}
    >
      <div className="pin-glow" aria-hidden="true" />
      <img
        src={src}
        alt=""
        draggable={false}
        onLoad={() => void revealPin(revealedRef)}
        ref={(image) => {
          if (image?.complete && image.naturalWidth) void revealPin(revealedRef);
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
