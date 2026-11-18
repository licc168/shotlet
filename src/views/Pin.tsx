import { getCurrentWindow } from "@tauri-apps/api/window";
import { useEffect, useRef, useState } from "react";
import { copyFile, imageSrc, isTauri, pinPath } from "../api";
import { useI18n } from "../i18n";

export function Pin() {
  const { t } = useI18n();
  const [src, setSrc] = useState("");
  const [scale, setScale] = useState(1);
  const [opacity, setOpacity] = useState(1);
  const [menu, setMenu] = useState(false);
  const pinRef = useRef<HTMLDivElement>(null);

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
        setOpacity((value) => Math.min(1, Math.max(0.15, value + (event.deltaY < 0 ? 0.05 : -0.05))));
      } else {
        setScale((value) => Math.min(4, Math.max(0.2, value + (event.deltaY < 0 ? 0.08 : -0.08))));
      }
    };
    node.addEventListener("wheel", onWheel, { passive: false });
    return () => node.removeEventListener("wheel", onWheel);
  }, [src]);

  function drag(event: React.PointerEvent) {
    if (!isTauri() || event.button !== 0) return;
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
      onDoubleClick={() => void closePin()}
      onContextMenu={(event) => { event.preventDefault(); setMenu(true); }}
    >
      <img src={imageSrc(src)} alt="" style={{ transform: `scale(${scale})` }} onPointerDown={drag} />
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
  const { getCurrentWindow } = await import("@tauri-apps/api/window");
  await getCurrentWindow().close();
}
