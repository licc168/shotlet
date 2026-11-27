import { useEffect, useRef, useState } from "react";
import { cancelScroll, finishScroll, scrollProgress, setScrollAuto } from "../api";
import { useI18n } from "../i18n";

function role(): string {
  return new URLSearchParams(window.location.search).get("role") ?? "hud";
}

export function ScrollChrome() {
  const { t } = useI18n();
  const kind = role();
  const frame = kind === "frame" || kind === "record";
  const [grown, setGrown] = useState(0);
  const [reverse, setReverse] = useState(false);
  const [auto, setAuto] = useState(false);
  const [canAuto, setCanAuto] = useState(false);
  const wantAuto = useRef<boolean | null>(null);
  const sawAuto = useRef(false);
  useEffect(() => {
    if (frame) return;
    const id = window.setInterval(() => {
      void scrollProgress().then((status) => {
        setGrown(status.height);
        setReverse(status.reverse);
        setCanAuto(status.canAuto);
        if (status.auto) sawAuto.current = true;
        // The capture loop turns auto off at the bottom. That has to clear the
        // button even if this page still remembers the click.
        if (!status.auto && (wantAuto.current == null || sawAuto.current)) {
          wantAuto.current = null;
          sawAuto.current = false;
          setAuto(false);
          return;
        }
        if (wantAuto.current == null || wantAuto.current === status.auto) {
          wantAuto.current = null;
          setAuto(status.auto);
        }
      }).catch(() => undefined);
    }, 200);
    return () => window.clearInterval(id);
  }, [frame]);
  function toggleAuto() {
    const next = !auto;
    wantAuto.current = next;
    setAuto(next);
    void setScrollAuto(next).catch(() => {
      if (wantAuto.current === next) {
        wantAuto.current = null;
        setAuto(!next);
      }
    });
  }
  if (frame) {
    return <div className={kind === "record" ? "record-frame" : "scroll-frame"} />;
  }
  const label = reverse ? t("scrollReverse") : grown > 0 ? `${t("scrollStitched")} ${grown}px` : t("scrollHint");
  return (
    <div className="scroll-hud">
      <span className={reverse ? "reverse" : ""}>{label}</span>
      {canAuto && (
        <button
          type="button"
          className={auto ? "on busy" : ""}
          title={auto ? t("scrollAutoStop") : t("scrollAuto")}
          onClick={toggleAuto}
        >
          {auto ? t("scrollScrolling") : t("scrollAuto")}
        </button>
      )}
      <button type="button" title={t("scrollEdit")} onClick={() => { void finishScroll("edit"); }}>{t("scrollEdit")}</button>
      <button type="button" title={t("scrollSave")} onClick={() => { void finishScroll("save"); }}>{t("scrollSave")}</button>
      <button type="button" className="danger" title={t("cancel")} onClick={() => { void cancelScroll(); }}>{t("cancel")}</button>
      <button type="button" className="check" title={t("scrollCopy")} onClick={() => { void finishScroll("copy"); }}>{t("scrollCopy")}</button>
    </div>
  );
}
