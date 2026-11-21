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
    let cancel = false;
    let stop: (() => void) | undefined;
    listHistory("", "all")
      .then((rows) => { if (!cancel) setItems(rows.slice(0, 6)); })
      .catch(() => { if (!cancel) setItems([]); });
    onEvent<string>("capture-error", (message) => { if (!cancel) setMessage(message); })
      .then((unlisten) => { if (cancel) unlisten(); else stop = unlisten; })
      .catch(() => undefined);
    return () => {
      cancel = true;
      stop?.();
    };
  }, []);

  const captureKeys = shortcutKeys(settings.captureShortcut);
  const pinKeys = shortcutKeys(settings.pinShortcut);

  return (
    <main className="home">
      <header className="home-top">
        <h1>{t("tagline")}</h1>
        <nav className="home-nav">
          <button className="ghost" onClick={() => void openHistory()}>{t("history")}</button>
          <button className="ghost" onClick={() => void openSettings()}>{t("settings")}</button>
        </nav>
      </header>
      <button className="primary home-capture" disabled={busy} onClick={() => {
        setBusy(true);
        setMessage("");
        startCapture().catch((error: unknown) => {
          setMessage(error instanceof Error ? error.message : String(error));
        }).finally(() => setBusy(false));
      }}>
        <span>{t("capture")}</span>
        {captureKeys.length > 0 && (
          <span className="home-keys">
            {captureKeys.map((key) => <kbd key={key}>{key}</kbd>)}
          </span>
        )}
      </button>
      {message && <p className="note">{message}</p>}
      <section className="recent">
        <div className="recent-head">
          <h2>{t("recent")}</h2>
          {pinKeys.length > 0 && (
            <p className="key-group">
              <span className="muted">{t("pinHotkey")}</span>
              {pinKeys.map((key) => <kbd key={key}>{key}</kbd>)}
            </p>
          )}
        </div>
        {items.length === 0 ? <p className="muted empty-note">{t("empty")}</p> : (
          <div className="thumbs">
            {items.map((item) => (
              <button key={item.id} className="thumb" title={item.name} onClick={() => void openHistory()}>
                <img src={imageSrc(item.file)} alt={item.name} />
                <span className="thumb-name">{item.name}</span>
                <small>{formatWhen(item.createdAt)}</small>
              </button>
            ))}
          </div>
        )}
      </section>
      <footer>{t("tray")}</footer>
    </main>
  );
}

function formatWhen(ms: number): string {
  const date = new Date(ms);
  if (Number.isNaN(date.getTime())) return "";
  const pad = (value: number) => String(value).padStart(2, "0");
  return `${pad(date.getMonth() + 1)}-${pad(date.getDate())} ${pad(date.getHours())}:${pad(date.getMinutes())}`;
}
