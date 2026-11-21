import { useEffect, useRef, useState } from "react";
import { commitHistoryRaw, copyFile, copyPngRaw, currentEditor, loadImage, ocrPng, pickSavePath, pinPngBase64, updateHistoryRaw, writePng } from "../api";
import { Annotator } from "../editor/Annotator";
import { rotateLayer, type Layer } from "../editor/model";
import { useI18n } from "../i18n";
import type { Rect } from "../lib/selection";

export function Editor() {
  const { t } = useI18n();
  const [image, setImage] = useState<HTMLImageElement | null>(null);
  const [recordId, setRecordId] = useState<string | null>(new URLSearchParams(location.search).get("id"));
  const [documentText, setDocumentText] = useState("[]");
  const [revision, setRevision] = useState(0);
  const [error, setError] = useState("");
  const sourcePath = useRef("");
  const fileBacked = useRef(true);
  const labels = useRef(t);
  labels.current = t;

  useEffect(() => {
    let dead = false;
    const id = new URLSearchParams(location.search).get("id");
    currentEditor(id).then(async (payload) => {
      if (dead) return;
      if (!payload) {
        setError(labels.current("empty"));
        return;
      }
      sourcePath.current = payload.path;
      fileBacked.current = true;
      setRecordId(payload.recordId);
      setDocumentText(payload.document);
      setImage(await loadImage(payload.path));
    }).catch((reason: unknown) => {
      if (!dead) setError(reason instanceof Error ? reason.message : String(reason));
    });
    return () => {
      dead = true;
    };
  }, []);

  if (!image) return <main className="home"><p>{error || t("busy")}</p></main>;

  function replaceImage(next: HTMLImageElement, layers: Layer[]) {
    setImage(next);
    setDocumentText(JSON.stringify(layers));
    setRevision((value) => value + 1);
  }

  return (
    <Annotator
      key={revision}
      image={image}
      selection={{ x: 0, y: 0, w: image.naturalWidth, h: image.naturalHeight }}
      mode="editor"
      initialDocument={documentText}
      onClose={() => window.close()}
      onPlainCopy={async () => {
        if (!fileBacked.current || !sourcePath.current) return false;
        await copyFile(sourcePath.current);
        return true;
      }}
      onRotate={(layers) => {
        fileBacked.current = false;
        const canvas = document.createElement("canvas");
        canvas.width = image.naturalHeight;
        canvas.height = image.naturalWidth;
        const ctx = canvas.getContext("2d");
        if (!ctx) return;
        ctx.translate(canvas.width, 0);
        ctx.rotate(Math.PI / 2);
        ctx.drawImage(image, 0, 0);
        const rotated = layers.map((layer) => rotateLayer(layer, image.naturalWidth, image.naturalHeight));
        const next = new Image();
        next.onload = () => replaceImage(next, rotated);
        next.src = canvas.toDataURL("image/png");
      }}
      onCrop={(rect) => {
        fileBacked.current = false;
        cropImage(image, rect).then((next) => replaceImage(next, [])).catch(() => undefined);
      }}
      onAction={async (action, png, base, layers, pngBytes, baseBytes) => {
        let ocrText = "";
        if (action === "ocr") ocrText = await ocrPng(pngBytes);
        if (action === "copy") await copyPngRaw(png, pngBytes);
        const body = { pngBase64: png, basePngBase64: base, document: JSON.stringify(layers), ocrText, width: image.naturalWidth, height: image.naturalHeight, name: "" };
        const item = recordId
          ? await updateHistoryRaw(recordId, body, pngBytes, baseBytes)
          : await commitHistoryRaw(body, pngBytes, baseBytes);
        if (!recordId) setRecordId(item.id);
        if (action === "save") {
          const dest = await pickSavePath(`${item.name}.png`);
          if (dest) await writePng(pngBytes, dest);
        }
        if (action === "pin") await pinPngBase64(png, 120, 120);
        return ocrText;
      }}
    />
  );
}

function cropImage(image: HTMLImageElement, rect: Rect): Promise<HTMLImageElement> {
  const canvas = document.createElement("canvas");
  canvas.width = Math.max(1, Math.round(rect.w));
  canvas.height = Math.max(1, Math.round(rect.h));
  const ctx = canvas.getContext("2d");
  if (!ctx) return Promise.reject(new Error("canvas"));
  ctx.drawImage(image, rect.x, rect.y, rect.w, rect.h, 0, 0, canvas.width, canvas.height);
  return new Promise((resolve, reject) => {
    const next = new Image();
    next.onload = () => resolve(next);
    next.onerror = () => reject(new Error("image"));
    next.src = canvas.toDataURL("image/png");
  });
}
