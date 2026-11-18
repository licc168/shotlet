import { createContext, useContext, useEffect, useMemo, useState, type ReactNode } from "react";
import { getSettings, saveSettings } from "./api";
import { defaultSettings, type Settings } from "./types";

const copy = {
  tagline: ["把屏幕上的一块钉住，边看边做。", "Pin a piece of the screen and keep working."],
  capture: ["截图", "Capture"],
  history: ["历史", "History"],
  settings: ["设置", "Settings"],
  recent: ["最近", "Recent"],
  empty: ["还没有截图。", "No captures yet."],
  tray: ["关掉窗口后仍在托盘，左键图标可以再打开。", "Closing this window leaves Shotlet in the tray."],
  cancel: ["取消", "Cancel"],
  copy: ["复制", "Copy"],
  save: ["保存", "Save"],
  pin: ["贴图", "Pin"],
  ocr: ["识别文字", "OCR"],
  scroll: ["滚动截图", "Scroll capture"],
  record: ["录制", "Record"],
  recordFormat: ["格式", "Format"],
  recordAudio: ["声音", "Audio"],
  recordMute: ["关", "Off"],
  recordMic: ["麦克风", "Microphone"],
  recordSpeaker: ["扬声器", "Speaker"],
  recordCursor: ["鼠标", "Pointer"],
  recordShow: ["显示", "Show"],
  recordHide: ["隐藏", "Hide"],
  recordStart: ["开始录制", "Start recording"],
  recordGifSilent: ["GIF 不录音", "GIF has no audio"],
  recordNeedFfmpeg: ["没有找到 ffmpeg，请改用 GIF 或先安装 ffmpeg", "ffmpeg was not found. Use GIF or install ffmpeg."],
  recordSpeakerHint: ["录的是正在播放的声音", "Captures what is playing"],
  undo: ["撤销", "Undo"],
  redo: ["重做", "Redo"],
  done: ["完成", "Done"],
  rect: ["矩形", "Rectangle"],
  ellipse: ["椭圆", "Ellipse"],
  arrow: ["箭头", "Arrow"],
  brush: ["画笔", "Brush"],
  mosaic: ["马赛克", "Mosaic"],
  highlight: ["高亮", "Highlight"],
  text: ["文字", "Text"],
  number: ["序号", "Number"],
  watermark: ["水印", "Watermark"],
  select: ["选择", "Select"],
  fill: ["填充", "Fill"],
  ellipseShape: ["椭圆高亮", "Ellipse highlight"],
  search: ["搜索名称、标签或图中文字", "Search name, tags, or text in the image"],
  all: ["全部", "All"],
  favorite: ["收藏", "Favorites"],
  deleted: ["最近删除", "Deleted"],
  grid: ["网格", "Grid"],
  list: ["列表", "List"],
  restore: ["恢复", "Restore"],
  delete: ["删除", "Delete"],
  purge: ["清空回收站", "Empty trash"],
  export: ["导出", "Export"],
  edit: ["再编辑", "Edit again"],
  tags: ["标签，用逗号分开", "Tags, separated by commas"],
  rename: ["名称", "Name"],
  theme: ["外观", "Appearance"],
  language: ["语言", "Language"],
  auto: ["跟随系统", "Follow system"],
  light: ["浅色", "Light"],
  dark: ["深色", "Dark"],
  hotkeys: ["快捷键", "Shortcuts"],
  captureHotkey: ["截屏", "Snip"],
  captureCopyHotkey: ["截屏并自动复制", "Snip and copy"],
  windowHotkey: ["自定义截屏", "Custom snip"],
  pinHotkey: ["贴图", "Pin"],
  hidePinsHotkey: ["隐藏/显示所有贴图", "Hide or show pins"],
  pinGroupHotkey: ["切换贴图组", "Switch pin group"],
  historyHotkey: ["打开历史", "Open history"],
  copyHotkey: ["复制最近一张", "Copy latest"],
  press: ["按下组合键", "Press a shortcut"],
  clear: ["清除", "Clear"],
  storage: ["保存位置", "Storage"],
  openFolder: ["打开文件夹", "Open folder"],
  saveSettings: ["保存设置", "Save settings"],
  saved: ["已保存", "Saved"],
  copied: ["已复制", "Copied"],
  pinned: ["已贴图", "Pinned"],
  ocrEmpty: ["没有识别到文字", "No text recognized"],
  ocrCopied: ["已复制识别出的文字", "Copied the recognized text"],
  hintPick: ["拖出区域后松手。按住 Shift 为正方形，双击窗口直接选中，Esc 取消。", "Drag a region and release. Shift makes a square, double-click selects a window, Esc cancels."],
  hintCopy: ["拖出区域后会直接复制到剪贴板。", "Release to copy the region to the clipboard."],
  recording: ["正在录制", "Recording"],
  stop: ["停止", "Stop"],
  crop: ["裁剪", "Crop"],
  rotate: ["旋转", "Rotate"],
  apply: ["应用", "Apply"],
  reference: ["交互参照 imjakey/shotlet，用 Tauri 重新实现。", "Interaction follows imjakey/shotlet, rebuilt with Tauri."],
  busy: ["请稍候", "Please wait"],
  noResult: ["没有匹配的截图", "No matching captures"],
  selectSome: ["先选中至少一张", "Select at least one capture"],
} as const;

export type CopyKey = keyof typeof copy;
export type Lang = "zh" | "en";

type I18nValue = {
  settings: Settings;
  lang: Lang;
  theme: "light" | "dark";
  t: (key: CopyKey) => string;
  update: (next: Settings) => Promise<void>;
  refresh: () => Promise<void>;
};

const I18n = createContext<I18nValue | null>(null);

function systemLang(): Lang {
  return navigator.language.toLowerCase().startsWith("zh") ? "zh" : "en";
}

function systemTheme(): "light" | "dark" {
  return window.matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light";
}

export function I18nProvider({ children }: { children: ReactNode }) {
  const [settings, setSettings] = useState<Settings>(defaultSettings);
  const [themeTick, setThemeTick] = useState(0);

  useEffect(() => {
    getSettings().then(setSettings).catch(() => setSettings(defaultSettings));
    const media = window.matchMedia("(prefers-color-scheme: dark)");
    const onChange = () => setThemeTick((value) => value + 1);
    media.addEventListener("change", onChange);
    return () => media.removeEventListener("change", onChange);
  }, []);

  const lang: Lang = settings.language === "zh" || settings.language === "en" ? settings.language : systemLang();
  const theme = settings.theme === "light" || settings.theme === "dark" ? settings.theme : systemTheme();

  useEffect(() => {
    document.documentElement.dataset.theme = theme;
    document.documentElement.lang = lang === "zh" ? "zh-CN" : "en";
  }, [theme, lang, themeTick]);

  const value = useMemo<I18nValue>(() => ({
    settings,
    lang,
    theme,
    t: (key) => copy[key][lang === "zh" ? 0 : 1],
    update: async (next) => {
      await saveSettings(next);
      setSettings(next);
    },
    refresh: async () => setSettings(await getSettings()),
  }), [settings, lang, theme]);

  return <I18n.Provider value={value}>{children}</I18n.Provider>;
}

export function useI18n(): I18nValue {
  const value = useContext(I18n);
  if (!value) throw new Error("i18n");
  return value;
}
