import { useEffect, useState } from "react";
import { isTauri, recordingStatus, stopRecording } from "../api";
import { useI18n } from "../i18n";

export function Recorder() {
  const { t } = useI18n();
  const [seconds, setSeconds] = useState(0);
  const [frames, setFrames] = useState(0);
  const [message, setMessage] = useState("");

  useEffect(() => {
    if (!isTauri()) return;
    void (async () => {
      const { getCurrentWindow, LogicalSize } = await import("@tauri-apps/api/window");
      const win = getCurrentWindow();
      await win.setSize(new LogicalSize(360, 112));
      await win.show();
      await win.setFocus();
    })();
  }, []);

  useEffect(() => {
    const timer = window.setInterval(() => {
      recordingStatus().then((status) => {
        setSeconds(Math.round(status.elapsedMs / 1000));
        setFrames(status.frames);
        if (status.message) setMessage(status.message);
      }).catch((error: unknown) => {
        setMessage(error instanceof Error ? error.message : String(error));
      });
    }, 300);
    return () => window.clearInterval(timer);
  }, []);

  return (
    <div className="recorder">
      <div className="recorder-main" onPointerDown={() => void drag()}>
        <span className="dot" />
        <strong>{t("recording")}</strong>
        <span>{seconds}s · {frames}</span>
      </div>
      <button type="button" onPointerDown={(event) => event.stopPropagation()} onClick={() => {
        setMessage(t("busy"));
        stopRecording().then(() => closeRecorder()).catch((error: unknown) => {
          setMessage(error instanceof Error ? error.message : String(error));
        });
      }}>{t("stop")}</button>
      {message && <small>{message}</small>}
    </div>
  );
}

async function drag() {
  if (!isTauri()) return;
  const { getCurrentWindow } = await import("@tauri-apps/api/window");
  await getCurrentWindow().startDragging();
}

async function closeRecorder() {
  if (!isTauri()) return;
  const { getCurrentWindow } = await import("@tauri-apps/api/window");
  await getCurrentWindow().close();
}
