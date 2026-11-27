import { useEffect, useState } from "react";
import { listAiModels, revealPath, storageDir, testAi } from "../api";
import { useI18n } from "../i18n";
import { AI_PRESETS } from "../lib/ai";
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
  const [testing, setTesting] = useState(false);
  const [aiMessage, setAiMessage] = useState("");
  const [tab, setTab] = useState<"general" | "ai">("general");
  const [models, setModels] = useState<string[]>([]);
  const [loadingModels, setLoadingModels] = useState(false);

  useEffect(() => setDraft(settings), [settings]);
  useEffect(() => { storageDir().then(setFolder).catch(() => setFolder("")); }, []);

  useEffect(() => {
    const endpoint = draft.aiEndpoint.trim();
    const key = draft.aiApiKey.trim();
    const local = draft.aiProvider === "ollama" || /localhost|127\.0\.0\.1/.test(endpoint);
    if (!endpoint || (!key && !local)) {
      setModels([]);
      return;
    }
    let cancel = false;
    const timer = window.setTimeout(() => {
      setLoadingModels(true);
      void listAiModels({
        provider: draft.aiProvider,
        endpoint: draft.aiEndpoint,
        apiKey: draft.aiApiKey,
        model: draft.aiModel,
      }).then((names) => {
        if (cancel) return;
        setModels(names);
        setDraft((current) => current.aiModel || !names[0] ? current : { ...current, aiModel: names[0] });
      }).catch((error: unknown) => {
        if (cancel) return;
        setModels([]);
        setAiMessage(error instanceof Error ? error.message : String(error));
      }).finally(() => {
        if (!cancel) setLoadingModels(false);
      });
    }, 400);
    return () => {
      cancel = true;
      window.clearTimeout(timer);
    };
  }, [draft.aiApiKey, draft.aiEndpoint, draft.aiProvider]);

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

  const modelChoices = draft.aiModel && !models.includes(draft.aiModel) ? [draft.aiModel, ...models] : models;

  return (
    <main className="settings">
      <div className="settings-tabs">
        <button className={tab === "general" ? "on" : ""} onClick={() => setTab("general")}>{t("general")}</button>
        <button className={tab === "ai" ? "on" : ""} onClick={() => setTab("ai")}>{t("ai")}</button>
      </div>
      {tab === "general" && <>
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
        <div className="hotkeys">
          {FIELDS.map((field) => (
            <div className="hotkey" key={field}>
              <span>{labels[field]}</span>
              <button className={recording === field ? "on key" : "key"} onClick={() => setRecording(field)}>
                {recording === field ? t("press") : (shortcutLabel(draft[field]) || "—")}
              </button>
              <button className="ghost" onClick={() => setDraft({ ...draft, [field]: "" })}>{t("clear")}</button>
            </div>
          ))}
        </div>
      </section>
      </>}
      {tab === "ai" && <section className="panel">
        <h2>{t("ai")}</h2>
        <div className="pair">
          <label>{t("aiProvider")}
            <select value={draft.aiProvider} onChange={(event) => {
              const provider = event.target.value;
              const preset = AI_PRESETS.find((item) => item.id === provider);
              setDraft({
                ...draft,
                aiProvider: provider,
                aiEndpoint: preset?.endpoint ?? "",
                aiModel: preset?.model ?? "",
              });
              setAiMessage("");
            }}>
              {AI_PRESETS.map((item) => <option key={item.id} value={item.id}>{item.label}</option>)}
            </select>
          </label>
          <label>{t("aiModel")}
            {modelChoices.length > 0 ? (
              <select value={draft.aiModel} onChange={(event) => setDraft({ ...draft, aiModel: event.target.value })}>
                {modelChoices.map((id) => <option key={id} value={id}>{id}</option>)}
              </select>
            ) : (
              <input value={draft.aiModel} spellCheck={false} placeholder={loadingModels ? t("aiModelsLoading") : "grok-4.7-build-fast"} onChange={(event) => setDraft({ ...draft, aiModel: event.target.value })} />
            )}
          </label>
        </div>
        <label>{t("aiEndpoint")}
          <input value={draft.aiEndpoint} spellCheck={false} placeholder="https://api.example.com/v1" onChange={(event) => setDraft({ ...draft, aiEndpoint: event.target.value })} />
        </label>
        <label>{t("aiKey")}
          <input type="password" value={draft.aiApiKey} autoComplete="off" spellCheck={false} onChange={(event) => setDraft({ ...draft, aiApiKey: event.target.value })} />
        </label>
        <p className="hint">{loadingModels ? t("aiModelsLoading") : t("aiHint")}</p>
        <div className="row">
          <button disabled={testing} onClick={() => {
            setTesting(true);
            setAiMessage("");
            void testAi({
              provider: draft.aiProvider,
              endpoint: draft.aiEndpoint,
              apiKey: draft.aiApiKey,
              model: draft.aiModel,
            }).then((text) => setAiMessage(text)).catch((error: unknown) => {
              setAiMessage(error instanceof Error ? error.message : String(error));
            }).finally(() => setTesting(false));
          }}>{testing ? t("aiTesting") : t("aiTest")}</button>
          {aiMessage && <span className="muted">{aiMessage}</span>}
        </div>
      </section>}
      {tab === "general" && (
        <section className="panel">
          <h2>{t("storage")}</h2>
          <input value={draft.storageDir} placeholder={folder} onChange={(event) => setDraft({ ...draft, storageDir: event.target.value })} />
          <div className="row">
            <button onClick={() => void revealPath(folder)}>{t("openFolder")}</button>
          </div>
        </section>
      )}
      <div className="savebar">
        <button className="primary" onClick={() => {
          update(draft).then(() => setMessage(t("saved"))).catch((error: unknown) => {
            setMessage(error instanceof Error ? error.message : String(error));
          });
        }}>{t("save")}</button>
        {message && <span className="muted">{message}</span>}
      </div>
    </main>
  );
}
