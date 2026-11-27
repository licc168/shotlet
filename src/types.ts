export type Settings = {
  theme: string;
  language: string;
  captureShortcut: string;
  captureCopyShortcut: string;
  windowShortcut: string;
  pinShortcut: string;
  hidePinsShortcut: string;
  pinGroupShortcut: string;
  historyShortcut: string;
  copyLatestShortcut: string;
  storageDir: string;
  aiProvider: string;
  aiEndpoint: string;
  aiApiKey: string;
  aiModel: string;
};

export type WindowInfo = {
  title: string;
  x: number;
  y: number;
  width: number;
  height: number;
  z: number;
};

export type PresetRect = {
  x: number;
  y: number;
  w: number;
  h: number;
};

export type ScreenImage = {
  x: number;
  y: number;
  width: number;
  height: number;
  path: string;
  screenX: number;
  screenY: number;
  pointWidth?: number;
  pointHeight?: number;
  windows?: WindowInfo[];
};

export type FrozenCapture = {
  path: string;
  width: number;
  height: number;
  originX: number;
  originY: number;
  windows: WindowInfo[];
  mode: string;
  preset: PresetRect | null;
  screens: ScreenImage[];
  generation: number;
};

export type HistoryItem = {
  id: string;
  createdAt: number;
  name: string;
  tags: string[];
  favorite: boolean;
  width: number;
  height: number;
  ocrText: string;
  file: string;
  deleted: boolean;
};

export type EditorPayload = {
  recordId: string | null;
  path: string;
  width: number;
  height: number;
  document: string;
};

export type CommitBody = {
  pngBase64: string;
  basePngBase64: string;
  document: string;
  ocrText: string;
  width: number;
  height: number;
  name: string;
};

export const defaultSettings: Settings = {
  theme: "auto",
  language: "auto",
  captureShortcut: "F1",
  captureCopyShortcut: "Control+F1",
  windowShortcut: "Shift+F1",
  pinShortcut: "F3",
  hidePinsShortcut: "Shift+F3",
  pinGroupShortcut: "Control+F3",
  historyShortcut: "Super+Shift+KeyH",
  copyLatestShortcut: "",
  storageDir: "",
  aiProvider: "deepseek",
  aiEndpoint: "https://api.deepseek.com/v1",
  aiApiKey: "",
  aiModel: "deepseek-v4-flash",
};
