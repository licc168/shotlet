import { useEffect, useState } from "react";
import { beginRecording, ffmpegAvailable, isTauri, recordingStatus, setRecordingPaused, stopRecording, type RecordAudio, type RecordingState } from "../api";
import { useI18n } from "../i18n";

const IDLE: RecordingState = {
  running: false,
  paused: false,
  done: false,
  ready: true,
  elapsedMs: 0,
  limitMs: 3_600_000,
  width: 0,
  height: 0,
  frames: 0,
  message: "",
};

export function Recorder() {
  const { t } = useI18n();
  const [status, setStatus] = useState<RecordingState>(IDLE);
  const [ffmpeg, setFfmpeg] = useState(false);
  const [format, setFormat] = useState<"gif" | "mp4">("mp4");
  const [speaker, setSpeaker] = useState(true);
  const [mic, setMic] = useState(true);
  const [cursor, setCursor] = useState(true);
  const [message, setMessage] = useState("");
  const [starting, setStarting] = useState(false);
  const [live, setLive] = useState(false);

  useEffect(() => {
    if (!isTauri()) return;
    void ffmpegAvailable().then((ready) => {
      setFfmpeg(ready);
      if (!ready) setFormat("gif");
    }).catch(() => setFormat("gif"));
    void resize(false);
  }, []);

  useEffect(() => {
    if (!isTauri()) return;
    const timer = window.setInterval(() => {
      recordingStatus().then((next) => {
        setStatus(next);
        if (next.message) setMessage(next.message);
      }).catch((error: unknown) => {
        setMessage(error instanceof Error ? error.message : String(error));
      });
    }, 300);
    return () => window.clearInterval(timer);
  }, []);

  useEffect(() => {
    if (!status.done) return;
    setMessage(t("recordSaving"));
    stopRecording().then(() => closeRecorder()).catch((error: unknown) => {
      setMessage(error instanceof Error ? error.message : String(error));
    });
  }, [status.done, t]);

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape" && !status.running) void closeRecorder();
      if (event.key === " " && status.running) {
        event.preventDefault();
        void setRecordingPaused(!status.paused);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [status.running, status.paused]);

  const audioOn = format === "mp4";
  const fps = format === "gif" ? 10 : 30;
  const width = status.width;
  const height = status.height;

  if (live || status.running || status.paused || status.done) {
    return (
      <div className="recorder recording">
        <div className="recorder-main" onPointerDown={() => void drag()}>
          <span className={status.paused ? "dot paused" : "dot"} />
          <strong>{clock(status.elapsedMs)}  /  {clock(status.limitMs || 3_600_000)}</strong>
        </div>
        <button type="button" className="pause" title={status.paused ? t("recordResume") : t("recordPause")} onPointerDown={(event) => event.stopPropagation()} onClick={() => {
          void setRecordingPaused(!status.paused);
        }}>{status.paused ? t("recordResume") : t("recordPause")}</button>
        <button type="button" className="stop" onPointerDown={(event) => event.stopPropagation()} onClick={() => {
          setMessage(t("recordSaving"));
          stopRecording().then(() => closeRecorder()).catch((error: unknown) => {
            setMessage(error instanceof Error ? error.message : String(error));
          });
        }}>{t("recordEnd")}</button>
        {message && <small>{message}</small>}
      </div>
    );
  }

  return (
    <div className="recorder ready">
      <button type="button" className="start" disabled={starting} onClick={() => {
        const audio: RecordAudio = !audioOn ? "none" : speaker && mic ? "both" : speaker ? "speaker" : mic ? "mic" : "none";
        setStarting(true);
        setMessage("");
        void beginRecording(format, audio, cursor).then(() => {
          setLive(true);
          return resize(true);
        }).catch((error: unknown) => {
          setMessage(error instanceof Error ? error.message : String(error));
        }).finally(() => setStarting(false));
      }}>{t("recordStart")}</button>
      <div className="choice">
        <span className="label">{t("recordFormat")}</span>
        <button type="button" className={format === "mp4" ? "seg on" : "seg"} disabled={!ffmpeg} title={ffmpeg ? "MP4" : t("recordNeedFfmpeg")} onClick={() => setFormat("mp4")}>MP4</button>
        <button type="button" className={format === "gif" ? "seg on" : "seg"} onClick={() => setFormat("gif")}>GIF</button>
      </div>
      <div className="toggles">
        <button type="button" className={speaker && audioOn ? "seg on" : "seg"} disabled={!audioOn} onClick={() => setSpeaker((value) => !value)}>{t("recordSpeaker")}</button>
        <button type="button" className={mic && audioOn ? "seg on" : "seg"} disabled={!audioOn} onClick={() => setMic((value) => !value)}>{t("recordMic")}</button>
        <button type="button" className={cursor ? "seg on" : "seg"} onClick={() => setCursor((value) => !value)}>{t("recordCursor")}</button>
      </div>
      <p className="meta">{format.toUpperCase()}{width > 0 ? ` · ${width} × ${height}` : ""} · {fps} FPS</p>
      {!ffmpeg && <p className="hint">{t("recordNeedFfmpeg")}</p>}
      {format === "gif" && <p className="hint">{t("recordGifSilent")}</p>}
      {audioOn && speaker && <p className="hint">{t("recordSpeakerHint")}</p>}
      {message && <small>{message}</small>}
      <button type="button" className="ghost" onClick={() => void closeRecorder()}>{t("cancel")}</button>
    </div>
  );
}

function clock(ms: number): string {
  const total = Math.max(0, Math.floor(ms / 1000));
  const hours = Math.floor(total / 3600);
  const minutes = Math.floor((total % 3600) / 60);
  const seconds = total % 60;
  return [hours, minutes, seconds].map((part) => String(part).padStart(2, "0")).join(":");
}

async function resize(recording: boolean) {
  if (!isTauri()) return;
  const { getCurrentWindow, LogicalSize } = await import("@tauri-apps/api/window");
  const win = getCurrentWindow();
  await win.setSize(new LogicalSize(400, recording ? 64 : 248));
  await win.show();
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
