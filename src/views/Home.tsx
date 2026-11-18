import { useEffect, useState } from "react";
import { imageSrc, listHistory, onEvent, openHistory, openSettings, startCapture } from "../api";
import { useI18n } from "../i18n";
import { shortcutKeys } from "../lib/shortcut";
import type { HistoryItem } from "../types";

export function Home() {
  const { t, settings } = useI18n();
  const [items, setItems] = useState<HistoryItem[]>([]);
  const [message, setMessage] = useState("");
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    listHistory("", "all").then((rows) => setItems(rows.slice(0, 6))).catch(() => setItems([]));
    let stop = () => undefined as void;
    onEvent<string>("capture-error", setMessage).then((unlisten) => { stop = unlisten; }).catch(() => undefined);
    return () => stop();
  }, []);

  return (
    <main className="home">
      <header>
        <p className="mark">Shotlet</p>
        <h1>{t("tagline")}</h1>
      </header>
      <button className="primary" disabled={busy} onClick={() => {
        setBusy(true);
        setMessage("");
        startCapture().catch((error: unknown) => {
          setMessage(error instanceof Error ? error.message : String(error));
        }).finally(() => setBusy(false));
      }}>{t("capture")}</button>
      <p className="keys">
        <span className="key-group">
          <span className="muted">{t("captureHotkey")}</span>
          {shortcutKeys(settings.captureShortcut).map((key, index) => <kbd key={`capture-${index}`}>{key}</kbd>)}
        </span>
        <span className="key-group">
          <span className="muted">{t("pinHotkey")}</span>
          {shortcutKeys(settings.pinShortcut).map((key, index) => <kbd key={`pin-${index}`}>{key}</kbd>)}
        </span>
      </p>
      <div className="row">
        <button onClick={() => void openHistory()}>{t("history")}</button>
        <button onClick={() => void openSettings()}>{t("settings")}</button>
      </div>
      <section className="recent">
        <h2>{t("recent")}</h2>
        {items.length === 0 ? <p className="muted empty-note">{t("empty")}</p> : (
          <div className="thumbs">
            {items.map((item) => (
              <button key={item.id} className="thumb" title={item.name} onClick={() => void openHistory()}>
                <img src={imageSrc(item.file)} alt={item.name} />
              </button>
            ))}
          </div>
        )}
      </section>
      {message && <p className="note">{message}</p>}
      <footer>{t("tray")}</footer>
    </main>
  );
}
