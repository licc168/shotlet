import { useEffect, useState } from "react";
import { copyFile, deleteCaptures, exportCaptures, imageSrc, listHistory, openEditorRecord, pickDirectory, pinFile, purgeDeleted, renameCapture, restoreCaptures, setFavorite, setTags } from "../api";
import { useI18n } from "../i18n";
import type { HistoryItem } from "../types";

export function History() {
  const { t } = useI18n();
  const [query, setQuery] = useState("");
  const [filter, setFilter] = useState("all");
  const [layout, setLayout] = useState<"grid" | "list">("grid");
  const [items, setItems] = useState<HistoryItem[]>([]);
  const [selected, setSelected] = useState<string[]>([]);
  const [message, setMessage] = useState("");

  async function reload() {
    setItems(await listHistory(query, filter));
  }

  useEffect(() => {
    reload().catch((error: unknown) => setMessage(error instanceof Error ? error.message : String(error)));
  }, [query, filter]);

  const current = items.find((item) => item.id === selected[selected.length - 1]);

  function toggle(id: string, event: React.MouseEvent) {
    if (event.metaKey || event.ctrlKey) {
      setSelected((ids) => ids.includes(id) ? ids.filter((item) => item !== id) : [...ids, id]);
      return;
    }
    setSelected([id]);
  }

  async function run(task: () => Promise<void>) {
    setMessage("");
    try {
      await task();
      await reload();
    } catch (error) {
      setMessage(error instanceof Error ? error.message : String(error));
    }
  }

  return (
    <main className="history">
      <header>
        <h1>{t("history")}</h1>
        <input value={query} placeholder={t("search")} onChange={(event) => setQuery(event.target.value)} />
      </header>
      <div className="filterbar">
        <div className="filters">
          {(["all", "favorite", "deleted"] as const).map((item) => (
            <button key={item} className={filter === item ? "on" : ""} onClick={() => setFilter(item)}>{t(item)}</button>
          ))}
        </div>
        <div className="filters">
          <button className={layout === "grid" ? "on" : ""} onClick={() => setLayout("grid")}>{t("grid")}</button>
          <button className={layout === "list" ? "on" : ""} onClick={() => setLayout("list")}>{t("list")}</button>
        </div>
      </div>
      <div className={layout === "grid" ? "cards" : "rows"}>
        {items.map((item) => (
          <article key={item.id} className={selected.includes(item.id) ? "on" : ""} onClick={(event) => toggle(item.id, event)}>
            <img src={imageSrc(item.file)} alt="" />
            <div>
              <strong>{item.name}</strong>
              <span>{item.tags.join(", ")}</span>
              <small>{new Date(item.createdAt).toLocaleString()}</small>
            </div>
          </article>
        ))}
        {items.length === 0 && <p className="muted empty-note">{t("noResult")}</p>}
      </div>
      {current && (
        <aside>
          <label>{t("rename")}<input value={current.name} onChange={(event) => {
            const name = event.target.value;
            setItems((rows) => rows.map((item) => item.id === current.id ? { ...item, name } : item));
          }} onBlur={() => void run(() => renameCapture(current.id, current.name))} /></label>
          <label>{t("tags")}<input value={current.tags.join(", ")} onChange={(event) => {
            const tags = event.target.value.split(",").map((tag) => tag.trim()).filter(Boolean);
            setItems((rows) => rows.map((item) => item.id === current.id ? { ...item, tags } : item));
          }} onBlur={() => void run(() => setTags(current.id, current.tags))} /></label>
          {current.ocrText && <p className="ocr">{current.ocrText}</p>}
          <div className="row">
            <button onClick={() => void run(() => setFavorite(current.id, !current.favorite))}>{current.favorite ? "★" : "☆"} {t("favorite")}</button>
            <button onClick={() => void copyFile(current.file)}>{t("copy")}</button>
            <button onClick={() => void pinFile(current.file, 80, 80)}>{t("pin")}</button>
            <button onClick={() => void openEditorRecord(current.id)}>{t("edit")}</button>
          </div>
        </aside>
      )}
      <footer>
        <button onClick={() => selected.length ? void run(() => deleteCaptures(selected)) : setMessage(t("selectSome"))}>{t("delete")}</button>
        <button onClick={() => selected.length ? void run(() => restoreCaptures(selected)) : setMessage(t("selectSome"))}>{t("restore")}</button>
        <button onClick={() => void run(() => purgeDeleted())}>{t("purge")}</button>
        <button onClick={() => void run(async () => {
          if (selected.length === 0) throw new Error(t("selectSome"));
          const dest = await pickDirectory();
          if (!dest) return;
          const count = await exportCaptures(selected, dest);
          setMessage(String(count));
        })}>{t("export")}</button>
        {message && <span>{message}</span>}
      </footer>
    </main>
  );
}
