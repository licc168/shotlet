import { useEffect, useState } from "react";
import { cancelScroll, finishScroll, scrollProgress } from "../api";
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
  useEffect(() => {
    if (frame) return;
    const id = window.setInterval(() => {
      void scrollProgress().then((status) => {
        setGrown(status.height);
        setReverse(status.reverse);
      }).catch(() => undefined);
    }, 200);
    return () => window.clearInterval(id);
  }, [frame]);
  if (frame) {
    return <div className={kind === "record" ? "record-frame" : "scroll-frame"} />;
  }
  const label = reverse ? t("scrollReverse") : grown > 0 ? `${t("scrollStitched")} ${grown}px` : t("scrollHint");
  return (
    <div className="scroll-hud">
      <span className={reverse ? "reverse" : ""}>{label}</span>
      <button type="button" title={t("scrollEdit")} onClick={() => { void finishScroll("edit"); }}>{t("scrollEdit")}</button>
      <button type="button" title={t("scrollSave")} onClick={() => { void finishScroll("save"); }}>{t("scrollSave")}</button>
      <button type="button" className="danger" title={t("cancel")} onClick={() => { void cancelScroll(); }}>{t("cancel")}</button>
      <button type="button" className="check" title={t("scrollCopy")} onClick={() => { void finishScroll("copy"); }}>{t("scrollCopy")}</button>
    </div>
  );
}
