import { useEffect, useState } from "react";
import { cancelScroll, finishScroll, scrollProgress } from "../api";
import { useI18n } from "../i18n";

function role(): string {
  return new URLSearchParams(window.location.search).get("role") ?? "hud";
}

export function ScrollChrome() {
  const { t } = useI18n();
  const frame = role() === "frame";
  const [grown, setGrown] = useState(0);
  useEffect(() => {
    if (frame) return;
    const id = window.setInterval(() => {
      void scrollProgress().then(setGrown).catch(() => undefined);
    }, 200);
    return () => window.clearInterval(id);
  }, [frame]);
  if (frame) {
    return <div className="scroll-frame" />;
  }
  return (
    <div className="scroll-hud">
      <span>{grown > 0 ? `${t("scrollStitched")} ${grown}px` : t("scrollHint")}</span>
      <button type="button" className="danger" onClick={() => { void cancelScroll(); }}>{t("cancel")}</button>
      <button type="button" className="check" onClick={() => { void finishScroll(); }}>{t("done")}</button>
    </div>
  );
}
