import { useEffect, useState } from "react";
import { revealPath, storageDir } from "../api";
import { useI18n } from "../i18n";
import { shortcutLabel } from "../lib/shortcut";
import type { Settings } from "../types";

const FIELDS = [
  "captureShortcut",
  "captureCopyShortcut",
  "windowShortcut",
  "pinShortcut",
  "hidePinsShortcut",
  "pinGroupShortcut",
  "historyShortcut",
] as const;

export function SettingsView() {
  const { t, settings, update } = useI18n();
  const [draft, setDraft] = useState<Settings>(settings);
  const [recording, setRecording] = useState<typeof FIELDS[number] | null>(null);
  const [folder, setFolder] = useState("");
  const [message, setMessage] = useState("");

  useEffect(() => setDraft(settings), [settings]);
  useEffect(() => { storageDir().then(setFolder).catch(() => setFolder("")); }, []);

  useEffect(() => {
    if (!recording) return;
    const field = recording;
    function onKey(event: KeyboardEvent) {
      event.preventDefault();
      if (event.key === "Escape") {
        setRecording(null);
        return;
      }
      const parts: string[] = [];
      if (event.ctrlKey) parts.push("Control");
      if (event.altKey) parts.push("Alt");
      if (event.shiftKey) parts.push("Shift");
      if (event.metaKey) parts.push("Super");
      if (["Control", "Alt", "Shift", "Meta"].includes(event.key)) return;
      const key = event.code.startsWith("Key") || event.code.startsWith("Digit") || event.code.startsWith("F")
        ? event.code
        : event.code;
      if (parts.length === 0 && !key.startsWith("F")) return;
      parts.push(key);
      setDraft((current) => ({ ...current, [field]: parts.join("+") }));
      setRecording(null);
    }
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [recording]);

  const labels = {
    captureShortcut: t("captureHotkey"),
    captureCopyShortcut: t("captureCopyHotkey"),
    windowShortcut: t("windowHotkey"),
    pinShortcut: t("pinHotkey"),
    hidePinsShortcut: t("hidePinsHotkey"),
    pinGroupShortcut: t("pinGroupHotkey"),
    historyShortcut: t("historyHotkey"),
  };

  return (
    <main className="settings">
      <header>
        <h1>{t("settings")}</h1>
      </header>
      <section className="panel">
        <div className="pair">
          <label>{t("theme")}
            <select value={draft.theme} onChange={(event) => setDraft({ ...draft, theme: event.target.value })}>
              <option value="auto">{t("auto")}</option>
              <option value="light">{t("light")}</option>
              <option value="dark">{t("dark")}</option>
            </select>
          </label>
          <label>{t("language")}
            <select value={draft.language} onChange={(event) => setDraft({ ...draft, language: event.target.value })}>
              <option value="auto">{t("auto")}</option>
              <option value="zh">简体中文</option>
              <option value="en">English</option>
            </select>
          </label>
        </div>
      </section>
      <section className="panel">
        <h2>{t("hotkeys")}</h2>
        {FIELDS.map((field) => (
          <div className="hotkey" key={field}>
            <span>{labels[field]}</span>
            <button className={recording === field ? "on key" : "key"} onClick={() => setRecording(field)}>
              {recording === field ? t("press") : (shortcutLabel(draft[field]) || t("clear"))}
            </button>
            <button className="ghost" onClick={() => setDraft({ ...draft, [field]: "" })}>{t("clear")}</button>
          </div>
        ))}
      </section>
      <section className="panel">
        <h2>{t("storage")}</h2>
        <p className="muted path">{folder || t("storage")}</p>
        <input value={draft.storageDir} placeholder={folder} onChange={(event) => setDraft({ ...draft, storageDir: event.target.value })} />
        <div className="row">
          <button onClick={() => void revealPath(folder)}>{t("openFolder")}</button>
        </div>
      </section>
      <div className="savebar">
        <button className="primary" onClick={() => {
          update(draft).then(() => setMessage(t("saved"))).catch((error: unknown) => {
            setMessage(error instanceof Error ? error.message : String(error));
          });
        }}>{t("saveSettings")}</button>
        {message && <p className="note">{message}</p>}
      </div>
      <footer>{t("reference")}</footer>
    </main>
  );
}
