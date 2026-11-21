import { lazy, Suspense } from "react";
import { I18nProvider } from "./i18n";
import "./styles.css";

const Editor = lazy(() => import("./views/Editor").then((m) => ({ default: m.Editor })));
const History = lazy(() => import("./views/History").then((m) => ({ default: m.History })));
const Home = lazy(() => import("./views/Home").then((m) => ({ default: m.Home })));
const Overlay = lazy(() => import("./views/Overlay").then((m) => ({ default: m.Overlay })));
const Pin = lazy(() => import("./views/Pin").then((m) => ({ default: m.Pin })));
const Recorder = lazy(() => import("./views/Recorder").then((m) => ({ default: m.Recorder })));
const ScrollChrome = lazy(() => import("./views/ScrollChrome").then((m) => ({ default: m.ScrollChrome })));
const SettingsView = lazy(() => import("./views/Settings").then((m) => ({ default: m.SettingsView })));

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
      <Suspense fallback={null}>
        {view === "overlay" && <Overlay />}
        {view === "history" && <History />}
        {view === "settings" && <SettingsView />}
        {view === "editor" && <Editor />}
        {view === "pin" && <Pin />}
        {view === "recorder" && <Recorder />}
        {view === "scroll" && <ScrollChrome />}
        {view === "home" && <Home />}
      </Suspense>
    </I18nProvider>
  );
}
