import { I18nProvider } from "./i18n";
import { Editor } from "./views/Editor";
import { History } from "./views/History";
import { Home } from "./views/Home";
import { Overlay } from "./views/Overlay";
import { Pin } from "./views/Pin";
import { Recorder } from "./views/Recorder";
import { SettingsView } from "./views/Settings";
import "./styles.css";

function viewName(): string {
  const query = new URLSearchParams(window.location.search).get("view");
  if (query) return query;
  const injected = (window as Window & { __SHOTLET_VIEW__?: string }).__SHOTLET_VIEW__;
  return injected || "home";
}

export default function App() {
  const view = viewName();
  return (
    <I18nProvider>
      {view === "overlay" && <Overlay />}
      {view === "history" && <History />}
      {view === "settings" && <SettingsView />}
      {view === "editor" && <Editor />}
      {view === "pin" && <Pin />}
      {view === "recorder" && <Recorder />}
      {view === "home" && <Home />}
    </I18nProvider>
  );
}
