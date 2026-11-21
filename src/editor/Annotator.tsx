import { useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { isDoubleClick } from "../lib/pin";
import { clampRect, contains, isDragGlitch, normalize, square, type Rect } from "../lib/selection";
import { useI18n } from "../i18n";
import { tracePage, type ShotImage } from "../api";
import { previewScale, renderPng, renderRgba, renderScene } from "./draw";
import { appendStrokePoint, COLORS, hitTest, layerBounds, MAX_FONT, MIN_FONT, moveLayer, parseLayers, resizeBox, resizeTextLayer, uid, type Layer } from "./model";

type Tool = "select" | "rect" | "ellipse" | "arrow" | "brush" | "mosaic" | "highlight" | "text" | "number" | "watermark";
type TextDraft = { id?: string; x: number; y: number; value: string };
type Action = "copy" | "save" | "pin" | "ocr";

type Props = {
  image: ShotImage;
  selection: Rect;
  onSelectionChange?: (rect: Rect) => void;
  editableSelection?: boolean;
  mode: "capture" | "editor";
  initialDocument?: string;
  desktop?: boolean;
  onClose: () => void;
  onAction: (
    action: Action,
    png: string,
    base: string,
    layers: Layer[],
    pngBytes: Uint8Array<ArrayBuffer>,
    baseBytes: Uint8Array<ArrayBuffer>,
  ) => Promise<string | void>;
  onPlainCopy?: (selection: Rect) => Promise<boolean>;
  onPlainPin?: (selection: Rect) => Promise<boolean>;
  onAnnotatedPin?: (pixels: Uint8ClampedArray<ArrayBuffer>, width: number, height: number, selection: Rect, layers: Layer[]) => Promise<boolean>;
  onScroll?: () => void;
  onRecord?: () => void;
  onRotate?: (layers: Layer[]) => void;
  onCrop?: (rect: Rect) => void;
};

const TOOLS: Tool[] = ["rect", "ellipse", "arrow", "brush", "mosaic", "highlight", "text", "number", "watermark", "select"];
const CAPTURE_DOUBLE_CLICK_MS = 500;
const CAPTURE_DOUBLE_CLICK_DIST = 48;
const NEW_SELECTION_SLOP = 6;

// WebView2 drops a canvas update made after paint until the next mouse move.
// Reading one pixel forces the frame through. Other WebViews do not need it,
// and the read stalls the GPU pipeline on every drag frame.
const needsCanvasFlush = typeof navigator !== "undefined" && /Edg\//i.test(navigator.userAgent);

export function Annotator(props: Props) {
  const { t } = useI18n();
  const stageRef = useRef<HTMLDivElement>(null);
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const barRef = useRef<HTMLDivElement>(null);
  const [frame, setFrame] = useState({ w: 0, h: 0 });
  const [barSize, setBarSize] = useState({ w: 680, h: 44 });
  const [tool, setTool] = useState<Tool>("select");
  const [color, setColor] = useState(COLORS[0]);
  const [thickness, setThickness] = useState(4);
  const [filled, setFilled] = useState(false);
  const [fontSize, setFontSize] = useState(28);
  const [block, setBlock] = useState(12);
  const [radius, setRadius] = useState(18);
  const [ellipseHighlight, setEllipseHighlight] = useState(false);
  const [watermark, setWatermark] = useState("Shotlet");
  const [layers, setLayers] = useState<Layer[]>(() => parseLayers(props.initialDocument ?? "[]"));
  const [undo, setUndo] = useState<Layer[][]>([]);
  const [redo, setRedo] = useState<Layer[][]>([]);
  const [selected, setSelected] = useState<string | null>(null);
  const [draft, setDraft] = useState<Layer | null>(null);
  const draftRef = useRef<Layer | null>(null);
  const [textDraft, setTextDraft] = useState<TextDraft | null>(null);
  const [numbers, setNumbers] = useState(1);
  const [busy, setBusy] = useState(false);
  const [note, setNote] = useState("");
  const [ocrText, setOcrText] = useState("");
  const [cropping, setCropping] = useState(false);
  const [crop, setCrop] = useState<Rect | null>(null);
  const drag = useRef<{ kind: string; id?: string; handle?: string; start: { x: number; y: number }; last: { x: number; y: number }; origin: Layer | null; box: Rect | null } | null>(null);
  const layersRef = useRef(layers);
  const selectedRef = useRef(selected);
  const fontSizeRef = useRef(fontSize);
  const colorRef = useRef(color);
  const textDraftRef = useRef(textDraft);
  const textAreaRef = useRef<HTMLTextAreaElement>(null);
  const caretRef = useRef({ start: 0, end: 0 });
  const keepTextFocus = useRef(false);
  const fontAtDown = useRef<number | null>(null);
  const watermarkAtFocus = useRef<string | null>(null);
  const lastPointerDown = useRef<{ time: number; x: number; y: number } | null>(null);
  type MoveSnapshot = { clientX: number; clientY: number; buttons: number; shiftKey: boolean };
  const pendingMove = useRef<MoveSnapshot | null>(null);
  const moveRaf = useRef(0);
  const framedSelection = useRef(props.selection);
  if (props.selection.w >= 4 && props.selection.h >= 4) framedSelection.current = props.selection;
  layersRef.current = layers;
  draftRef.current = draft;
  selectedRef.current = selected;
  fontSizeRef.current = fontSize;
  colorRef.current = color;
  textDraftRef.current = textDraft;
  const editingId = textDraft?.id;

  function commitDraft(next: Layer | null) {
    draftRef.current = next;
    setDraft(next);
  }

  const shown = useMemo(() => {
    const visible = editingId ? layers.filter((layer) => layer.id !== editingId) : layers;
    return draft ? [...visible, draft] : visible;
  }, [layers, draft, editingId]);

  useLayoutEffect(() => {
    const stage = stageRef.current;
    if (!stage) return;
    const measure = () => {
      const rect = stage.getBoundingClientRect();
      setFrame((prev) => (prev.w === rect.width && prev.h === rect.height ? prev : { w: rect.width, h: rect.height }));
      const bar = barRef.current;
      if (bar) setBarSize((prev) => (prev.w === bar.offsetWidth && prev.h === bar.offsetHeight ? prev : { w: bar.offsetWidth, h: bar.offsetHeight }));
    };
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(stage);
    return () => observer.disconnect();
  }, [tool, props.mode]);

  useLayoutEffect(() => {
    const canvas = canvasRef.current;
    const ctx = canvas?.getContext("2d");
    if (!canvas || !ctx) return;
    if (shown.length === 0) {
      canvas.width = 1;
      canvas.height = 1;
      return;
    }
    const rect = canvas.getBoundingClientRect();
    const viewScale = previewScale(props.selection.w, props.selection.h, rect.width, rect.height, window.devicePixelRatio);
    try {
      renderScene(ctx, props.image, props.selection, shown, viewScale, false);
      if (needsCanvasFlush) ctx.getImageData(0, 0, 1, 1);
    } catch (error) {
      console.error(error);
    }
  }, [props.image, props.selection, shown, frame.w, frame.h]);

  function push(next: Layer[]) {
    const previous = layersRef.current;
    layersRef.current = next;
    setUndo((stack) => [...stack.slice(-40), previous]);
    setRedo([]);
    setLayers(next);
  }

  function choose(id: string | null, size?: number) {
    selectedRef.current = id;
    setSelected(id);
    if (size == null) return;
    fontSizeRef.current = size;
    setFontSize(size);
  }

  function changeColor(next: string) {
    colorRef.current = next;
    setColor(next);
    if (textDraftRef.current) return;
    const updated = layersRef.current.map((item) => {
      if (tool === "watermark" && item.type === "watermark" && item.color !== next) return { ...item, color: next };
      if (item.type === "text" && item.id === selectedRef.current && item.color !== next) return { ...item, color: next };
      if (item.type === "highlight" && item.id === selectedRef.current && item.color !== next) return { ...item, color: next };
      return item;
    });
    if (updated.some((item, index) => item !== layersRef.current[index])) push(updated);
  }

  function setHighlightEllipse(ellipse: boolean) {
    setEllipseHighlight(ellipse);
    const id = selectedRef.current;
    const updated = layersRef.current.map((item) => {
      if (item.type !== "highlight" || item.id !== id || item.ellipse === ellipse) return item;
      return { ...item, ellipse };
    });
    if (updated.some((item, index) => item !== layersRef.current[index])) push(updated);
  }

  function patchWatermarkText(text: string) {
    setLayers((items) => {
      let changed = false;
      const updated = items.map((item) => {
        if (item.type !== "watermark" || item.text === text) return item;
        changed = true;
        return { ...item, text };
      });
      if (!changed) return items;
      layersRef.current = updated;
      return updated;
    });
  }

  function rememberWatermarkText() {
    const mark = layersRef.current.find((item) => item.type === "watermark");
    watermarkAtFocus.current = mark?.type === "watermark" ? mark.text : null;
  }

  function commitWatermarkText() {
    const previous = watermarkAtFocus.current;
    watermarkAtFocus.current = null;
    if (previous == null) return;
    const mark = layersRef.current.find((item) => item.type === "watermark");
    if (!mark || mark.type !== "watermark" || mark.text === previous) return;
    const reverted = layersRef.current.map((item) => item.type === "watermark" ? { ...item, text: previous } : item);
    setUndo((stack) => [...stack.slice(-40), reverted]);
    setRedo([]);
  }

  function changeFontSize(next: number) {
    fontSizeRef.current = next;
    setFontSize(next);
    setLayers((items) => {
      let changed = false;
      const updated = items.map((item) => {
        if (tool === "watermark" && item.type === "watermark" && item.size !== next) {
          changed = true;
          return { ...item, size: next };
        }
        if (tool !== "watermark" && item.type === "text" && item.id === selectedRef.current && item.id !== textDraftRef.current?.id && item.size !== next) {
          changed = true;
          return { ...item, size: next };
        }
        return item;
      });
      if (!changed) return items;
      layersRef.current = updated;
      return updated;
    });
  }

  function rememberFontSize() {
    fontAtDown.current = fontSizeRef.current;
  }

  function restoreLayers(next: Layer[]) {
    layersRef.current = next;
    setLayers(next);
    const current = next.find((item) => item.id === selectedRef.current);
    const sized = current?.type === "text" ? current : tool === "watermark" ? next.find((item) => item.type === "watermark") : null;
    if (!sized || (sized.type !== "text" && sized.type !== "watermark")) return;
    fontSizeRef.current = sized.size;
    setFontSize(sized.size);
  }

  function commitFontSize() {
    const previous = fontAtDown.current;
    fontAtDown.current = null;
    const currentSize = fontSizeRef.current;
    if (previous == null || previous === currentSize) {
      releaseTextFocus();
      return;
    }
    const id = selectedRef.current;
    const mark = tool === "watermark";
    const current = layersRef.current;
    const reverted = current.map((item) => {
      if (mark && item.type === "watermark") return { ...item, size: previous };
      if (!mark && item.id === id && item.type === "text") return { ...item, size: previous };
      return item;
    });
    if (!reverted.every((item, index) => item === current[index])) {
      setUndo((stack) => [...stack.slice(-40), reverted]);
      setRedo([]);
    }
    releaseTextFocus();
  }

  function holdTextFocus() {
    if (textDraftRef.current) keepTextFocus.current = true;
  }

  function rememberCaret(area: HTMLTextAreaElement) {
    caretRef.current = { start: area.selectionStart ?? 0, end: area.selectionEnd ?? 0 };
  }

  function focusText() {
    const area = textAreaRef.current;
    if (!area || !textDraftRef.current) return;
    if (document.activeElement === area) return;
    area.focus();
    const caret = caretRef.current;
    area.setSelectionRange(caret.start, caret.end);
  }

  function releaseTextFocus() {
    if (!keepTextFocus.current) return;
    keepTextFocus.current = false;
    focusText();
  }

  function toImage(event: { clientX: number; clientY: number }) {
    const stage = stageRef.current;
    if (!stage) return { x: 0, y: 0 };
    const rect = stage.getBoundingClientRect();
    return {
      x: ((event.clientX - rect.left) / rect.width) * props.image.naturalWidth,
      y: ((event.clientY - rect.top) / rect.height) * props.image.naturalHeight,
    };
  }

  function scale() {
    const stage = stageRef.current;
    if (!stage) return 1;
    return stage.getBoundingClientRect().width / props.image.naturalWidth;
  }

  function onPointerDown(event: React.PointerEvent) {
    if (textDraft || busy) return;
    if (event.button === 0 && props.mode === "capture") {
      const stamp = { time: event.timeStamp, x: event.clientX, y: event.clientY };
      const repeated = event.detail >= 2 || isDoubleClick(lastPointerDown.current, stamp, CAPTURE_DOUBLE_CLICK_MS, CAPTURE_DOUBLE_CLICK_DIST);
      lastPointerDown.current = stamp;
      const target = event.target;
      const onControl = target instanceof Element && Boolean(target.closest("textarea, input, button, select, .toolbar, .subbar, .record-panel"));
      if (repeated && !onControl) {
        drag.current = null;
        const framed = framedSelection.current;
        const selection = framed.w >= 4 && framed.h >= 4 ? framed : props.selection;
        void run("copy", selection);
        return;
      }
    }
    const point = toImage(event);
    if (cropping) {
      drag.current = { kind: "crop", start: point, last: point, origin: null, box: null };
      setCrop(normalize(point.x, point.y, point.x, point.y));
      return;
    }
    const handle = selected ? handleAt(point) : null;
    if (handle && selected) {
      const layer = layers.find((item) => item.id === selected) ?? null;
      drag.current = { kind: "resize", id: selected, handle, start: point, last: point, origin: layer, box: layer ? layerBounds(layer) : null };
      capturePointer(event);
      return;
    }
    if (tool === "select") {
      const hitLayer = layerAt(point);
      choose(hitLayer?.id ?? null, hitLayer?.type === "text" ? hitLayer.size : undefined);
      if (hitLayer) {
        drag.current = { kind: "move", id: hitLayer.id, start: point, last: point, origin: hitLayer, box: null };
        capturePointer(event);
        return;
      }
      const edge = props.editableSelection ? selectionHandleAt(point) : null;
      if (edge && props.onSelectionChange) {
        drag.current = { kind: "resize-selection", handle: edge, start: point, last: point, origin: null, box: props.selection };
        return;
      }
      if (props.editableSelection && props.onSelectionChange && !contains(props.selection, point.x, point.y)) {
        drag.current = { kind: "new-selection", start: point, last: point, origin: null, box: props.selection };
        return;
      }
      if (props.editableSelection && contains(props.selection, point.x, point.y)) {
        drag.current = { kind: "move-selection", start: point, last: point, origin: null, box: props.selection };
      }
      return;
    }
    if (!contains(props.selection, point.x, point.y)) return;
    if (tool === "text") {
      const hitLayer = layerAt(point);
      if (hitLayer?.type === "text") {
        choose(hitLayer.id, hitLayer.size);
        drag.current = { kind: "move", id: hitLayer.id, start: point, last: point, origin: hitLayer, box: null };
        capturePointer(event);
        return;
      }
      choose(null);
      if (!textDraft) {
        event.preventDefault();
        const next = { x: point.x, y: point.y, value: "" };
        caretRef.current = { start: 0, end: 0 };
        textDraftRef.current = next;
        setTextDraft(next);
      }
      return;
    }
    if (tool === "number") {
      push([...layers, { id: uid(), type: "number", x: point.x, y: point.y, n: numbers, color }]);
      setNumbers((value) => value + 1);
      return;
    }
    if (tool === "watermark") {
      const without = layers.filter((layer) => layer.type !== "watermark");
      push([...without, { id: uid(), type: "watermark", text: watermark || "Shotlet", color, size: fontSize }]);
      return;
    }
    const id = uid();
      const created = createDraft(id, tool, point, color, thickness, filled, block, radius, ellipseHighlight);
    if (created) {
      commitDraft(created);
      drag.current = { kind: "draw", id, start: point, last: point, origin: created, box: null };
      capturePointer(event);
    }
  }

  function applyMove(event: MoveSnapshot) {
    const current = drag.current;
    if (!current || (event.buttons & 1) === 0) {
      if (!current && props.editableSelection && stageRef.current) {
        stageRef.current.style.cursor = resizeCursor(selectionHandleAt(toImage(event)));
      }
      return;
    }
    const raw = toImage(event);
    if (isDragGlitch(raw, current.last, { w: props.image.naturalWidth, h: props.image.naturalHeight })) return;
    current.last = raw;
    const point = raw;
    if (current.kind === "crop") {
      const next = event.shiftKey ? square(current.start.x, current.start.y, point.x, point.y) : normalize(current.start.x, current.start.y, point.x, point.y);
      setCrop(next);
      return;
    }
    if (current.kind === "draw" && current.origin) {
      if (current.origin.type === "brush" || current.origin.type === "mosaic") {
        const next = appendStrokePoint(current.origin, point);
        if (next !== current.origin) {
          current.origin = next;
          commitDraft(next);
        }
        return;
      }
      commitDraft(updateDraft(current.origin, current.start, point, event.shiftKey));
      return;
    }
    if (current.kind === "move" && current.origin) {
      const dx = point.x - current.start.x;
      const dy = point.y - current.start.y;
      const moved = moveLayer(current.origin, dx, dy);
      setLayers((items) => {
        const updated = items.map((item) => item.id === current.origin?.id ? moved : item);
        layersRef.current = updated;
        return updated;
      });
      return;
    }
    if (current.kind === "resize" && current.box && current.handle && current.origin) {
      if (stageRef.current) stageRef.current.style.cursor = resizeCursor(current.handle);
      const box = resizeBox(current.box, current.handle, point.x, point.y);
      const next = applyBox(current.origin, box, current.handle, point);
      if (next.type === "text") {
        fontSizeRef.current = next.size;
        setFontSize(next.size);
      }
      setLayers((items) => {
        const updated = items.map((item) => item.id === current.origin?.id ? next : item);
        layersRef.current = updated;
        return updated;
      });
      return;
    }
    if (current.kind === "resize-selection" && current.box && current.handle && props.onSelectionChange) {
      if (stageRef.current) stageRef.current.style.cursor = resizeCursor(current.handle);
      const next = clampRect(
        resizeBox(current.box, current.handle, point.x, point.y),
        { x: 0, y: 0, w: props.image.naturalWidth, h: props.image.naturalHeight },
      );
      props.onSelectionChange(next);
      return;
    }
    if (current.kind === "new-selection" && props.onSelectionChange) {
      if (Math.hypot(point.x - current.start.x, point.y - current.start.y) * scale() < NEW_SELECTION_SLOP) return;
      const next = clampRect(
        event.shiftKey ? square(current.start.x, current.start.y, point.x, point.y) : normalize(current.start.x, current.start.y, point.x, point.y),
        { x: 0, y: 0, w: props.image.naturalWidth, h: props.image.naturalHeight },
      );
      props.onSelectionChange(next);
      return;
    }
    if (current.kind === "move-selection" && current.box && props.onSelectionChange) {
      const dx = point.x - current.start.x;
      const dy = point.y - current.start.y;
      const next = clampRect(
        { ...current.box, x: current.box.x + dx, y: current.box.y + dy },
        { x: 0, y: 0, w: props.image.naturalWidth, h: props.image.naturalHeight },
      );
      props.onSelectionChange(next);
    }
  }

  function flushMove() {
    if (moveRaf.current) {
      cancelAnimationFrame(moveRaf.current);
      moveRaf.current = 0;
    }
    const snapshot = pendingMove.current;
    pendingMove.current = null;
    if (snapshot) applyMove(snapshot);
  }

  // High-frequency pointers fire far more often than the display refreshes.
  // Coalesce to one update per frame: keep only the newest snapshot and let the
  // layout effect render once, instead of a full re-render per event.
  function onPointerMove(event: React.PointerEvent) {
    pendingMove.current = { clientX: event.clientX, clientY: event.clientY, buttons: event.buttons, shiftKey: event.shiftKey };
    if (moveRaf.current) return;
    moveRaf.current = requestAnimationFrame(() => {
      moveRaf.current = 0;
      const snapshot = pendingMove.current;
      pendingMove.current = null;
      if (snapshot) applyMove(snapshot);
    });
  }

  useEffect(() => () => {
    if (moveRaf.current) cancelAnimationFrame(moveRaf.current);
  }, []);

  function onPointerUp(event: React.PointerEvent) {
    flushMove();
    const current = drag.current;
    drag.current = null;
    if (stageRef.current) stageRef.current.style.cursor = "";
    if (current?.kind === "new-selection" && current.box && props.onSelectionChange && (props.selection.w < 4 || props.selection.h < 4)) {
      props.onSelectionChange(current.box);
    }
    if (current?.kind === "draw") {
      let finished = draftRef.current;
      if (current.origin && (current.origin.type === "brush" || current.origin.type === "mosaic")) {
        const point = toImage(event);
        finished = isDragGlitch(point, current.last, { w: props.image.naturalWidth, h: props.image.naturalHeight })
          ? current.origin
          : appendStrokePoint(current.origin, point);
      }
      if (finished) {
        const box = layerBounds(finished);
        if (box.w > 2 || box.h > 2 || finished.type === "brush" || finished.type === "mosaic") {
          push([...layers, finished]);
          choose(finished.id);
          if (finished.type === "highlight") {
            colorRef.current = finished.color ?? colorRef.current;
            setColor(finished.color ?? colorRef.current);
            setEllipseHighlight(finished.ellipse);
          }
        }
        commitDraft(null);
      }
    }
    if (current?.kind === "move" && current.origin?.type === "text") {
      const distance = Math.hypot(current.last.x - current.start.x, current.last.y - current.start.y);
      if (distance < 4) {
        const origin = current.origin;
        setLayers((items) => {
          const updated = items.map((item) => item.id === origin.id ? origin : item);
          layersRef.current = updated;
          return updated;
        });
        editText(origin);
        return;
      }
    }
    if (current?.kind === "move" || current?.kind === "resize") {
      setUndo((stack) => [...stack.slice(-40), undoSnapshot(current.origin, layersRef.current)]);
      setRedo([]);
    }
  }

  function undoSnapshot(origin: Layer | null, current: Layer[]): Layer[] {
    if (!origin) return current;
    return current.map((item) => item.id === origin.id ? origin : item);
  }

  function layerAt(point: { x: number; y: number }): Layer | null {
    const hit = hitTest(layers, point.x, point.y);
    return layers.find((item) => item.id === hit) ?? null;
  }

  function handleAt(point: { x: number; y: number }): string | null {
    const layer = layers.find((item) => item.id === selected);
    if (!layer || layer.type === "watermark" || layer.type === "brush" || layer.type === "mosaic") return null;
    const pad = 8 / scale();
    if (layer.type === "arrow") {
      if (Math.hypot(point.x - layer.x1, point.y - layer.y1) < pad * 1.4) return "start";
      if (Math.hypot(point.x - layer.x2, point.y - layer.y2) < pad * 1.4) return "end";
      return null;
    }
    const box = layerBounds(layer);
    if (layer.type === "text") {
      const corners: Array<[string, number, number]> = [
        ["nw", box.x, box.y], ["ne", box.x + box.w, box.y], ["sw", box.x, box.y + box.h], ["se", box.x + box.w, box.y + box.h],
      ];
      return corners.find(([, x, y]) => Math.abs(point.x - x) <= pad && Math.abs(point.y - y) <= pad)?.[0] ?? null;
    }
    return spotAt(point, box, pad);
  }

  function selectionHandleAt(point: { x: number; y: number }): string | null {
    return spotAt(point, props.selection, 10 / scale());
  }

  function spotAt(point: { x: number; y: number }, box: Rect, pad: number): string | null {
    const spots: Array<[string, number, number]> = [
      ["nw", box.x, box.y], ["ne", box.x + box.w, box.y], ["sw", box.x, box.y + box.h], ["se", box.x + box.w, box.y + box.h],
      ["n", box.x + box.w / 2, box.y], ["s", box.x + box.w / 2, box.y + box.h], ["w", box.x, box.y + box.h / 2], ["e", box.x + box.w, box.y + box.h / 2],
    ];
    return spots.find(([, x, y]) => Math.abs(point.x - x) <= pad && Math.abs(point.y - y) <= pad)?.[0] ?? null;
  }

  function editText(layer: Extract<Layer, { type: "text" }>) {
    colorRef.current = layer.color;
    setColor(layer.color);
    fontSizeRef.current = layer.size;
    setFontSize(layer.size);
    caretRef.current = { start: layer.text.length, end: layer.text.length };
    const next = { id: layer.id, x: layer.x, y: layer.y, value: layer.text };
    textDraftRef.current = next;
    setTextDraft(next);
    choose(layer.id, layer.size);
  }

  function commitText() {
    const current = textDraftRef.current;
    if (!current) return;
    textDraftRef.current = null;
    setTextDraft(null);
    const value = current.value;
    const size = fontSizeRef.current;
    const ink = colorRef.current;
    if (!value.trim()) {
      if (current.id) {
        push(layersRef.current.filter((layer) => layer.id !== current.id));
        choose(null);
      }
      return;
    }
    if (current.id) {
      const previous = layersRef.current.find((layer) => layer.id === current.id);
      if (previous?.type === "text" && previous.text === value && previous.color === ink && previous.size === size && previous.x === current.x && previous.y === current.y) return;
      push(layersRef.current.map((layer) => layer.id === current.id && layer.type === "text" ? { ...layer, text: value, color: ink, size } : layer));
      choose(current.id, size);
      return;
    }
    const layer: Layer = { id: uid(), type: "text", x: current.x, y: current.y, text: value, color: ink, size };
    push([...layersRef.current, layer]);
    choose(layer.id, layer.size);
  }

  function blurText(event: React.FocusEvent<HTMLTextAreaElement>) {
    rememberCaret(event.currentTarget);
    if (keepTextFocus.current) return;
    const next = event.relatedTarget;
    if (next instanceof Element && next.closest(".subbar")) return;
    commitText();
  }

  async function run(action: Action, selection = props.selection) {
    setBusy(true);
    setNote("");
    try {
      commitText();
      const marks = layersRef.current;
      if (action === "copy" && marks.length === 0 && props.onPlainCopy) {
        const handled = await props.onPlainCopy(selection);
        if (handled) return;
      }
      if (action === "pin" && marks.length === 0 && props.onPlainPin) {
        const handled = await props.onPlainPin(selection);
        if (handled) return;
      }
      if (action === "pin" && marks.length > 0 && props.onAnnotatedPin) {
        try {
          tracePage("pin: render begin");
          const frame = renderRgba(props.image, selection, marks);
          tracePage("pin: render done");
          const handled = await props.onAnnotatedPin(frame.pixels, frame.width, frame.height, selection, marks);
          if (handled) return;
        } catch {
          // Binary bodies are not available everywhere. The base64 path below still pins.
        }
      }
      const plain = marks.length === 0;
      const needsBase = action !== "pin" && !plain;
      const baseTask = needsBase ? renderPng(props.image, selection, [], false) : null;
      if (action === "pin") tracePage("pin: render begin");
      const rendered = await renderPng(props.image, selection, marks, true);
      if (action === "pin") tracePage("pin: render done");
      // `pin` keeps an empty Base64 base for the legacy path, but the binary
      // frame still needs a real comparison image, so it uses the shot itself.
      // Otherwise reuse the same render when plain, or the un-annotated one.
      const baseResult = action === "pin"
        ? { base64: "", bytes: rendered.bytes }
        : (baseTask ? await baseTask : rendered);
      const text = await props.onAction(action, rendered.base64, baseResult.base64, marks, rendered.bytes, baseResult.bytes);
      if (action === "ocr") {
        const value = typeof text === "string" ? text.trim() : "";
        setOcrText(value);
        if (!value) setNote(t("ocrEmpty"));
        else {
          try {
            await navigator.clipboard.writeText(value);
          } catch {
            // The desktop command already placed the text on the clipboard when it could.
          }
          setNote(t("ocrCopied"));
        }
      } else if (typeof text === "string") {
        setOcrText(text);
      }
    } catch (error) {
      setNote(error instanceof Error ? error.message : String(error));
    } finally {
      setBusy(false);
    }
  }

  const onKeyRef = useRef<(event: KeyboardEvent) => void>(() => {});
  onKeyRef.current = (event: KeyboardEvent) => {
    const target = event.target as HTMLElement | null;
    if (target && (target.tagName === "TEXTAREA" || target.tagName === "INPUT")) return;
    if (event.key === "Escape") props.onClose();
    if (event.key === "Enter") void run("copy");
    if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "z") {
      event.preventDefault();
      if (event.shiftKey) {
        setRedo((stack) => {
          const previous = stack[stack.length - 1];
          if (!previous) return stack;
          setUndo((items) => [...items, layers]);
          restoreLayers(previous);
          return stack.slice(0, -1);
        });
      } else {
        setUndo((stack) => {
          const previous = stack[stack.length - 1];
          if (!previous) return stack;
          setRedo((items) => [...items, layers]);
          restoreLayers(previous);
          return stack.slice(0, -1);
        });
      }
    }
    if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "s") {
      event.preventDefault();
      void run("save");
    }
    if (event.key === "Delete" || event.key === "Backspace") {
      if (selected) push(layers.filter((layer) => layer.id !== selected));
      choose(null);
    }
  };
  useEffect(() => {
    const handler = (event: KeyboardEvent) => onKeyRef.current(event);
    window.addEventListener("keydown", handler);
    return () => window.removeEventListener("keydown", handler);
  }, []);

  const stageStyle = props.desktop
    ? { position: "absolute" as const, inset: 0, width: "100%", height: "100%" }
    : { width: props.image.naturalWidth, height: props.image.naturalHeight, maxWidth: "100%", maxHeight: "calc(100vh - 120px)" };
  const box = selectionCss(props.selection, props.image, frame, Boolean(props.desktop));
  const selectedLayer = layers.find((item) => item.id === selected) ?? null;
  const textSelected = selectedLayer?.type === "text";
  const textScreen = selectedLayer?.type === "text" ? layerScreenBox(layerBounds(selectedLayer), props.selection, box) : null;
  const dock = props.desktop ? dockBar(props.selection, props.image, barSize.w, barSize.h) : null;
  const dockStyle = dock ? { left: dock.left, top: dock.top, bottom: "auto", transform: "none" } : undefined;

  return (
    <div className={props.desktop ? "anno-layer" : "editor-root"}>
      <div className="stage" ref={stageRef} style={stageStyle} onPointerDown={onPointerDown} onPointerMove={onPointerMove} onPointerUp={onPointerUp} onPointerCancel={onPointerUp}>
        {!props.desktop && <img src={props.image.src} alt="" draggable={false} />}
        <Shade box={box} />
        <canvas ref={canvasRef} className="anno" hidden={shown.length === 0} style={{ left: box.left, top: box.top, width: box.width, height: box.height }} />
        <div className="hole" style={{ left: box.left, top: box.top, width: box.width, height: box.height }} />
        {props.editableSelection && <SelectionHandles box={box} />}
        {selected && !textDraft && <Handles layer={selectedLayer} box={textScreen ?? box} />}
        {textScreen && !textDraft && <div className="layer-box" style={textScreen} />}
        {crop && <div className="crop" style={selectionCss(crop, props.image, frame, Boolean(props.desktop))} />}
        {textDraft && (
          <textarea
            key={textDraft.id ?? `${textDraft.x}:${textDraft.y}`}
            ref={textAreaRef}
            autoFocus
            className="text-draft"
            style={{
              left: box.left + (textDraft.x - props.selection.x) * (box.width / props.selection.w),
              top: box.top + (textDraft.y - props.selection.y) * (box.height / props.selection.h),
              color,
              fontSize: fontSize * (box.width / Math.max(props.selection.w, 1)),
            }}
            value={textDraft.value}
            onChange={(event) => {
              const next = { ...textDraft, value: event.target.value };
              textDraftRef.current = next;
              setTextDraft(next);
              rememberCaret(event.currentTarget);
            }}
            onSelect={(event) => rememberCaret(event.currentTarget)}
            onKeyUp={(event) => rememberCaret(event.currentTarget)}
            onMouseUp={(event) => rememberCaret(event.currentTarget)}
            onFocus={(event) => {
              const caret = caretRef.current;
              event.currentTarget.setSelectionRange(caret.start, caret.end);
            }}
            onKeyDown={(event) => {
              if (event.key === "Enter" && !event.shiftKey) {
                event.preventDefault();
                event.stopPropagation();
                commitText();
              }
              if (event.key === "Escape") {
                event.stopPropagation();
                textDraftRef.current = null;
                setTextDraft(null);
              }
            }}
            onBlur={blurText}
          />
        )}
        <div className="size-label" style={{ left: box.left, top: Math.max(8, box.top - 28) }}>{Math.round(props.selection.w)} × {Math.round(props.selection.h)}</div>
      </div>
      <div className="toolbar" ref={barRef} style={dockStyle}>
        {TOOLS.map((item) => (
          <button key={item} className={tool === item ? "on" : ""} title={t(item)} onClick={() => {
            setTool(item);
            if (item !== "watermark") return;
            const mark = layers.find((layer) => layer.type === "watermark");
            if (mark?.type !== "watermark") return;
            fontSizeRef.current = mark.size;
            setFontSize(mark.size);
          }}><ToolIcon name={item} /></button>
        ))}
        <span className="split" />
        <button title={t("undo")} onClick={() => {
          const previous = undo[undo.length - 1];
          if (!previous) return;
          setRedo((stack) => [...stack, layers]);
          setUndo((stack) => stack.slice(0, -1));
          restoreLayers(previous);
        }}><ToolIcon name="undo" /></button>
        <button title={t("redo")} onClick={() => {
          const previous = redo[redo.length - 1];
          if (!previous) return;
          setUndo((stack) => [...stack, layers]);
          setRedo((stack) => stack.slice(0, -1));
          restoreLayers(previous);
        }}><ToolIcon name="redo" /></button>
        <span className="split" />
        {props.mode === "capture" && props.onScroll && <button title={t("scroll")} onClick={props.onScroll}><ToolIcon name="scroll" /></button>}
        {props.mode === "capture" && props.onRecord && (
          <button title={t("record")} onClick={() => props.onRecord?.()}><ToolIcon name="record" /></button>
        )}
        {props.mode === "editor" && props.onRotate && <button title={t("rotate")} onClick={() => props.onRotate?.(layers)}><ToolIcon name="rotate" /></button>}
        {props.mode === "editor" && props.onCrop && (
          <button title={t("crop")} className={cropping ? "on wide" : "wide"} onClick={() => { setCropping((value) => !value); setCrop(null); }}>{t("crop")}</button>
        )}
        {cropping && crop && props.onCrop && <button className="wide" onClick={() => props.onCrop?.(crop)}>{t("apply")}</button>}
        <button className="wide" title={t("ocr")} onClick={() => void run("ocr")}>OCR</button>
        <span className="split" />
        <button title={t("pin")} onClick={() => void run("pin")}><ToolIcon name="pin" /></button>
        <button title={t("save")} onClick={() => void run("save")}><ToolIcon name="save" /></button>
        <button className="danger" title={t("cancel")} onClick={props.onClose}><ToolIcon name="close" /></button>
        <button className="check" title={t("copy")} onClick={() => void run("copy")}><ToolIcon name="check" /></button>
      </div>
      {((tool !== "select" && tool !== "number") || textSelected) && (
        <div className="subbar" onPointerDown={holdTextFocus} onPointerUp={releaseTextFocus} onPointerCancel={releaseTextFocus} style={dock ? { left: dock.left, top: dock.subTop, bottom: "auto", transform: "none" } : undefined}>
          {(tool !== "select" && tool !== "number" || textSelected || textDraft) && COLORS.map((swatch) => (
            <button key={swatch} className={(selectedLayer?.type === "highlight" ? selectedLayer.color ?? color : color) === swatch ? "swatch on" : "swatch"} style={{ background: swatch }} onMouseDown={(event) => event.preventDefault()} onClick={() => changeColor(swatch)} />
          ))}
          {(tool === "rect" || tool === "ellipse" || tool === "arrow" || tool === "brush") && (
            <label>{thickness}<input type="range" min={2} max={16} value={thickness} onChange={(event) => setThickness(Number(event.target.value))} /></label>
          )}
          {(tool === "rect" || tool === "ellipse") && <label><input type="checkbox" checked={filled} onChange={(event) => setFilled(event.target.checked)} />{t("fill")}</label>}
          {tool === "highlight" && <label><input type="checkbox" checked={selectedLayer?.type === "highlight" ? selectedLayer.ellipse : ellipseHighlight} onChange={(event) => setHighlightEllipse(event.target.checked)} />{t("ellipseShape")}</label>}
          {(tool === "text" || tool === "watermark" || textSelected) && (
            <label>{fontSize}<input type="range" min={MIN_FONT} max={MAX_FONT} value={fontSize} onPointerDown={rememberFontSize} onPointerUp={commitFontSize} onLostPointerCapture={commitFontSize} onChange={(event) => changeFontSize(Number(event.target.value))} /></label>
          )}
          {tool === "watermark" && (
            <input
              value={watermark}
              onFocus={rememberWatermarkText}
              onBlur={commitWatermarkText}
              onChange={(event) => {
                const text = event.target.value;
                setWatermark(text);
                patchWatermarkText(text);
              }}
            />
          )}
          {tool === "mosaic" && (
            <>
              <label>{block}<input type="range" min={4} max={32} value={block} onChange={(event) => setBlock(Number(event.target.value))} /></label>
              <label>{radius}<input type="range" min={8} max={48} value={radius} onChange={(event) => setRadius(Number(event.target.value))} /></label>
            </>
          )}
        </div>
      )}
      {(note || ocrText || busy) && (
        <div className="note ocr-result">
          {busy ? <span>{t("busy")}</span> : (
            <>
              {note && <b>{note}</b>}
              {ocrText && <span>{ocrText}</span>}
            </>
          )}
        </div>
      )}
    </div>
  );
}

function selectionCss(selection: Rect, image: ShotImage, frame: { w: number; h: number }, desktop: boolean) {
  const width = frame.w || (desktop ? window.innerWidth : image.naturalWidth);
  const height = frame.h || (desktop ? window.innerHeight : image.naturalHeight);
  const sx = width / Math.max(image.naturalWidth, 1);
  const sy = height / Math.max(image.naturalHeight, 1);
  return { left: selection.x * sx, top: selection.y * sy, width: Math.max(0, selection.w * sx), height: Math.max(0, selection.h * sy) };
}

function dockBar(selection: Rect, image: ShotImage, barW: number, barH: number) {
  const viewW = Math.max(window.innerWidth, 1);
  const viewH = Math.max(window.innerHeight, 1);
  const sx = viewW / Math.max(image.naturalWidth, 1);
  const sy = viewH / Math.max(image.naturalHeight, 1);
  const selLeft = selection.x * sx;
  const selTop = selection.y * sy;
  const selWidth = selection.w * sx;
  const selHeight = selection.h * sy;
  const margin = 10;
  const width = Math.min(Math.max(barW, 1), Math.max(120, viewW - margin * 2));
  let left = selLeft + selWidth / 2 - width / 2;
  left = Math.max(margin, Math.min(left, viewW - margin - width));
  let top = selTop + selHeight + 12;
  if (top + barH > viewH - margin) top = selTop - 12 - barH;
  if (top < margin) top = Math.max(margin, viewH - margin - barH);
  let subTop = top - 48;
  if (subTop < margin) subTop = top + barH + 8;
  return { left, top, subTop };
}

function Shade({ box }: { box: { left: number; top: number; width: number; height: number } }) {
  if (box.width < 1 || box.height < 1) return null;
  return (
    <>
      <div className="shade top" style={{ height: Math.max(0, box.top) }} />
      <div className="shade left" style={{ top: box.top, width: Math.max(0, box.left), height: box.height }} />
      <div className="shade right" style={{ top: box.top, left: box.left + box.width, height: box.height }} />
      <div className="shade bottom" style={{ top: box.top + box.height }} />
    </>
  );
}

function resizeCursor(handle: string | null): string {
  if (handle === "n" || handle === "s") return "ns-resize";
  if (handle === "e" || handle === "w") return "ew-resize";
  if (handle === "ne" || handle === "sw") return "nesw-resize";
  if (handle === "nw" || handle === "se") return "nwse-resize";
  return "";
}

function SelectionHandles(props: { box: { left: number; top: number; width: number; height: number } }) {
  const names = ["nw", "n", "ne", "e", "se", "s", "sw", "w"];
  return (
    <>
      {names.map((name) => {
        const spot = edgePoint(name, props.box);
        return <i key={name} className={`handle ${name}`} style={{ left: spot.x, top: spot.y }} />;
      })}
    </>
  );
}

function edgePoint(name: string, box: { left: number; top: number; width: number; height: number }) {
  const x = name.includes("w") ? box.left : name.includes("e") ? box.left + box.width : box.left + box.width / 2;
  const y = name.includes("n") ? box.top : name.includes("s") ? box.top + box.height : box.top + box.height / 2;
  return { x, y };
}

function Handles(props: { layer: Layer | null; box: { left: number; top: number; width: number; height: number } }) {
  if (!props.layer || props.layer.type === "watermark" || props.layer.type === "brush" || props.layer.type === "mosaic") return null;
  const names = props.layer.type === "arrow" ? [] : ["nw", "ne", "sw", "se"];
  return (
    <>
      {names.map((name) => {
        const spot = corner(name, props.box);
        return <i key={name} className={`handle ${name}`} style={{ left: spot.x, top: spot.y }} />;
      })}
    </>
  );
}

function corner(name: string, box: { left: number; top: number; width: number; height: number }) {
  const x = name.includes("w") ? box.left : box.left + box.width;
  const y = name.includes("n") ? box.top : box.top + box.height;
  return { x, y };
}

function ToolIcon({ name }: { name: string }) {
  const stroke = { fill: "none", stroke: "currentColor", strokeWidth: 1.8, strokeLinecap: "round" as const, strokeLinejoin: "round" as const };
  return (
    <svg viewBox="0 0 24 24" width="18" height="18" aria-hidden="true">
      {name === "select" && <path {...stroke} d="M5 4.5v12.2l3.6-3.4 2.6 5.2 1.8-.9-2.6-5.2H17z" />}
      {name === "rect" && <rect {...stroke} x="4.5" y="6" width="15" height="12" rx="1.2" />}
      {name === "ellipse" && <circle {...stroke} cx="12" cy="12" r="6.6" />}
      {name === "arrow" && <path {...stroke} d="M5 19 19 5M19 5h-6M19 5v6" />}
      {name === "brush" && <path {...stroke} d="m14.2 4.8 5 5L9.2 19.8H4.2v-5zM14.2 4.8l5 5" />}
      {name === "mosaic" && (
        <>
          <rect {...stroke} x="4" y="4" width="6.5" height="6.5" rx="0.6" />
          <rect {...stroke} x="13.5" y="4" width="6.5" height="6.5" rx="0.6" />
          <rect {...stroke} x="4" y="13.5" width="6.5" height="6.5" rx="0.6" />
          <rect {...stroke} x="13.5" y="13.5" width="6.5" height="6.5" rx="0.6" />
        </>
      )}
      {name === "highlight" && <rect x="5" y="7" width="14" height="10" rx="1.2" fill="currentColor" opacity="0.85" />}
      {name === "text" && <text x="12" y="16.5" textAnchor="middle" fontSize="13" fontWeight="650" fill="currentColor">T</text>}
      {name === "number" && (
        <>
          <circle {...stroke} cx="12" cy="12" r="7" />
          <path {...stroke} d="M11 9.2c.5-.6 1-.9 1.6-.9.2 0 .4 0 .4.1v7.4" />
        </>
      )}
      {name === "watermark" && (
        <>
          <rect {...stroke} x="4" y="6" width="16" height="12" rx="1.2" />
          <path {...stroke} d="M7 10.5h10M7 14h7" />
        </>
      )}
      {name === "undo" && <path {...stroke} d="M8 8H15.5A5.5 5.5 0 1 1 15.5 19H9M8 8 4.5 4.5M8 8 4.5 11.5" />}
      {name === "redo" && <path {...stroke} d="M16 8H8.5A5.5 5.5 0 1 0 8.5 19H15M16 8l3.5-3.5M16 8l3.5 3.5" />}
      {name === "scroll" && <path {...stroke} d="M12 4v16M8 8l4-4 4 4M8 16l4 4 4-4" />}
      {name === "record" && <circle cx="12" cy="12" r="5" fill="#e23b32" />}
      {name === "rotate" && <path {...stroke} d="M19 12a7 7 0 1 1-2-4.9M19 4v4h-4" />}
      {name === "pin" && (
        <>
          <path {...stroke} d="M12 17v5" />
          <path {...stroke} d="M9 10.76a2 2 0 0 1-1.11 1.79l-1.78.9A2 2 0 0 0 5 15.24V16a1 1 0 0 0 1 1h12a1 1 0 0 0 1-1v-.76a2 2 0 0 0-1.11-1.79l-1.78-.9A2 2 0 0 1 15 10.76V7a1 1 0 0 1 1-1 2 2 0 0 0 0-4H8a2 2 0 0 0 0 4 1 1 0 0 1 1 1z" />
        </>
      )}
      {name === "save" && <path {...stroke} d="M12 4v10M8 10l4 4 4-4M5 19h14" />}
      {name === "close" && <path {...stroke} d="M7 7l10 10M17 7 7 17" />}
      {name === "check" && <path d="M6 12.5 10 16.5 18 7.5" fill="none" stroke="currentColor" strokeWidth="2.2" strokeLinecap="round" strokeLinejoin="round" />}
    </svg>
  );
}

function createDraft(id: string, tool: Tool, point: { x: number; y: number }, color: string, thickness: number, filled: boolean, block: number, radius: number, ellipse: boolean): Layer | null {
  switch (tool) {
    case "rect":
      return { id, type: "rect", x: point.x, y: point.y, w: 0, h: 0, color, thickness, filled };
    case "ellipse":
      return { id, type: "ellipse", x: point.x, y: point.y, w: 0, h: 0, color, thickness, filled };
    case "arrow":
      return { id, type: "arrow", x1: point.x, y1: point.y, x2: point.x, y2: point.y, color, thickness };
    case "brush":
      return { id, type: "brush", points: [point], color, thickness };
    case "mosaic":
      return { id, type: "mosaic", points: [point], radius, block };
    case "highlight":
      return { id, type: "highlight", x: point.x, y: point.y, w: 0, h: 0, ellipse, color };
    default:
      return null;
  }
}

function capturePointer(event: React.PointerEvent) {
  const target = event.currentTarget;
  if (target instanceof Element && !target.hasPointerCapture(event.pointerId)) {
    target.setPointerCapture(event.pointerId);
  }
}

function updateDraft(origin: Layer, start: { x: number; y: number }, point: { x: number; y: number }, shift: boolean): Layer {
  if (origin.type === "arrow") return { ...origin, x2: point.x, y2: point.y };
  if (origin.type === "rect" || origin.type === "ellipse" || origin.type === "highlight") {
    const rect = shift ? square(start.x, start.y, point.x, point.y) : normalize(start.x, start.y, point.x, point.y);
    return { ...origin, ...rect };
  }
  return origin;
}

function applyBox(origin: Layer, box: Rect, handle: string | undefined, point: { x: number; y: number }): Layer {
  if (origin.type === "arrow") {
    if (handle === "start") return { ...origin, x1: point.x, y1: point.y };
    if (handle === "end") return { ...origin, x2: point.x, y2: point.y };
  }
  if (origin.type === "rect" || origin.type === "ellipse" || origin.type === "highlight") return { ...origin, ...box };
  if (origin.type === "text" && handle) return resizeTextLayer(origin, box, handle);
  if (origin.type === "number") return { ...origin, x: box.x, y: box.y };
  return origin;
}

function layerScreenBox(bounds: Rect, selection: Rect, view: { left: number; top: number; width: number; height: number }) {
  const sx = view.width / Math.max(selection.w, 1);
  const sy = view.height / Math.max(selection.h, 1);
  return {
    left: view.left + (bounds.x - selection.x) * sx,
    top: view.top + (bounds.y - selection.y) * sy,
    width: Math.max(0, bounds.w * sx),
    height: Math.max(0, bounds.h * sy),
  };
}

