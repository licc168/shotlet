use std::collections::HashMap;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::{OnceLock, RwLock};

pub use shotlet_types::{CommitRequest, HistoryItem, Record};
use shotlet_platform::{decode_png, err, load_rgba, new_id};

fn index_cache() -> &'static RwLock<HashMap<PathBuf, Vec<Record>>> {
    static CACHE: OnceLock<RwLock<HashMap<PathBuf, Vec<Record>>>> = OnceLock::new();
    CACHE.get_or_init(|| RwLock::new(HashMap::new()))
}

fn with_read<T>(root: &Path, body: impl FnOnce(&[Record]) -> Result<T, String>) -> Result<T, String> {
    {
        let guard = index_cache().read().map_err(|error| error.to_string())?;
        if let Some(records) = guard.get(root) {
            return body(records);
        }
    }
    let mut guard = index_cache().write().map_err(|error| error.to_string())?;
    if let Some(records) = guard.get(root) {
        return body(records);
    }
    let records = read_index(root)?;
    let result = body(&records)?;
    guard.insert(root.to_path_buf(), records);
    Ok(result)
}

fn with_write<T>(root: &Path, body: impl FnOnce(&mut Vec<Record>) -> Result<T, String>) -> Result<T, String> {
    let mut guard = index_cache().write().map_err(|error| error.to_string())?;
    let mut records = if let Some(records) = guard.get(root) {
        records.clone()
    } else {
        read_index(root)?
    };
    let result = body(&mut records)?;
    store_all(root, &records)?;
    guard.insert(root.to_path_buf(), records);
    Ok(result)
}

fn write_bytes(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(err)?;
    }
    std::fs::write(path, bytes).map_err(err)
}

/// Identical png and base share one inode. A failed link falls back to a second write.
fn write_linked(png_path: &Path, base_path: &Path, png: &[u8], base: &[u8]) -> Result<(), String> {
    write_bytes(png_path, png)?;
    if png != base {
        return write_bytes(base_path, base);
    }
    if base_path == png_path {
        return Ok(());
    }
    if base_path.exists() {
        let _ = std::fs::remove_file(base_path);
    }
    match std::fs::hard_link(png_path, base_path) {
        Ok(()) => Ok(()),
        Err(_) => write_bytes(base_path, base),
    }
}

fn png_payload(data: &str) -> Result<Vec<u8>, String> {
    let bytes = decode_png(data)?;
    if bytes.starts_with(b"\x89PNG") {
        return Ok(bytes);
    }
    shotlet_platform::encode_png_fast(&load_rgba(&bytes)?)
}

fn index_path(root: &Path) -> PathBuf {
    root.join("index.json")
}

fn read_index(root: &Path) -> Result<Vec<Record>, String> {
    let path = index_path(root);
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(err(error)),
    };
    match serde_json::from_str(&text) {
        Ok(records) => Ok(records),
        Err(error) => {
            let backup = path.with_file_name("index.json.broken");
            let _ = std::fs::copy(&path, &backup);
            Err(format!("历史索引已损坏，原文件还在，已备份为 index.json.broken：{error}"))
        }
    }
}

fn store_all(root: &Path, records: &[Record]) -> Result<(), String> {
    std::fs::create_dir_all(root).map_err(err)?;
    let text = serde_json::to_string_pretty(records).map_err(err)?;
    let path = index_path(root);
    let tmp = root.join("index.json.tmp");
    std::fs::write(&tmp, text).map_err(err)?;
    std::fs::rename(&tmp, path).map_err(err)
}

pub fn list(root: &Path, query: &str, filter: &str) -> Result<Vec<HistoryItem>, String> {
    let needle = query.trim().to_lowercase();
    with_read(root, |records| {
        Ok(records
            .iter()
            .filter(|record| record_matches(record, &needle, filter))
            .cloned()
            .map(to_item)
            .collect())
    })
}

pub fn record_matches(record: &Record, needle: &str, filter: &str) -> bool {
    let deleted = record.deleted_at.is_some();
    let filter_ok = match filter {
        "deleted" => deleted,
        "favorite" => record.favorite && !deleted,
        _ => !deleted,
    };
    if !filter_ok {
        return false;
    }
    if needle.is_empty() {
        return true;
    }
    let tags = record.tags.join(" ").to_lowercase();
    record.name.to_lowercase().contains(needle)
        || record.ocr_text.to_lowercase().contains(needle)
        || tags.contains(needle)
}

fn to_item(record: Record) -> HistoryItem {
    HistoryItem {
        deleted: record.deleted_at.is_some(),
        id: record.id,
        created_at: record.created_at,
        name: record.name,
        tags: record.tags,
        favorite: record.favorite,
        width: record.width,
        height: record.height,
        ocr_text: record.ocr_text,
        file: record.file,
    }
}

pub fn commit(root: &Path, request: CommitRequest) -> Result<HistoryItem, String> {
    let png = png_payload(&request.png_base64)?;
    let base = png_payload(&request.base_png_base64)?;
    commit_bytes(root, &png, &base, &request.document, request.width, request.height, &request.name, &request.ocr_text)
}

pub fn commit_image(root: &Path, image: &image::RgbaImage, name: &str) -> Result<HistoryItem, String> {
    let png = shotlet_platform::encode_png_fast(image)?;
    commit_bytes(root, &png, &png, "[]", image.width(), image.height(), name, "")
}

pub fn commit_bytes(
    root: &Path,
    png: &[u8],
    base: &[u8],
    document: &str,
    width: u32,
    height: u32,
    name: &str,
    ocr_text: &str,
) -> Result<HistoryItem, String> {
    with_write(root, |records| {
        let id = new_id();
        let images = root.join("images");
        let docs = root.join("docs");
        let file = images.join(format!("{id}.png"));
        let base_file = images.join(format!("{id}.base.png"));
        let doc_file = docs.join(format!("{id}.json"));
        write_linked(&file, &base_file, png, base)?;
        write_bytes(&doc_file, document.as_bytes())?;
        let label = if name.trim().is_empty() { format!("截图 {id}") } else { name.to_string() };
        let record = Record {
            id,
            created_at: chrono::Utc::now().timestamp_millis(),
            name: label,
            tags: Vec::new(),
            favorite: false,
            width,
            height,
            ocr_text: ocr_text.to_string(),
            file: file.to_string_lossy().to_string(),
            base_file: base_file.to_string_lossy().to_string(),
            document: doc_file.to_string_lossy().to_string(),
            deleted_at: None,
        };
        records.insert(0, record.clone());
        Ok(to_item(record))
    })
}

pub fn update_document(root: &Path, id: &str, request: CommitRequest) -> Result<HistoryItem, String> {
    let png = png_payload(&request.png_base64)?;
    let base = png_payload(&request.base_png_base64)?;
    with_write(root, |records| update_locked(records, id, &request, &png, &base))
}

/// Same update without a Base64 round trip. Used by the binary IPC command.
#[allow(clippy::too_many_arguments)]
pub fn update_document_bytes(
    root: &Path,
    id: &str,
    png: &[u8],
    base: &[u8],
    document: &str,
    width: u32,
    height: u32,
    name: &str,
    ocr_text: &str,
) -> Result<HistoryItem, String> {
    let request = CommitRequest {
        png_base64: String::new(),
        base_png_base64: String::new(),
        document: document.to_string(),
        ocr_text: ocr_text.to_string(),
        width,
        height,
        name: name.to_string(),
    };
    with_write(root, |records| update_locked(records, id, &request, png, base))
}

fn update_locked(records: &mut Vec<Record>, id: &str, request: &CommitRequest, png: &[u8], base: &[u8]) -> Result<HistoryItem, String> {
    let Some(record) = records.iter_mut().find(|record| record.id == id) else {
        return Err("找不到这条历史记录".into());
    };
    write_linked(Path::new(&record.file), Path::new(&record.base_file), png, base)?;
    write_bytes(Path::new(&record.document), request.document.as_bytes())?;
    record.width = request.width;
    record.height = request.height;
    record.ocr_text = request.ocr_text.clone();
    if !request.name.trim().is_empty() {
        record.name = request.name.clone();
    }
    Ok(to_item(record.clone()))
}

pub fn editor_source(root: &Path, id: &str) -> Result<(String, u32, u32, String), String> {
    let record = with_read(root, |records| {
        records
            .iter()
            .find(|record| record.id == id)
            .cloned()
            .ok_or_else(|| "找不到这条历史记录".to_string())
    })?;
    let document = std::fs::read_to_string(&record.document).unwrap_or_else(|_| "[]".into());
    Ok((record.base_file, record.width, record.height, document))
}

pub fn latest_file(root: &Path) -> Result<Option<String>, String> {
    with_read(root, |records| {
        Ok(records.iter().find(|record| record.deleted_at.is_none()).map(|record| record.file.clone()))
    })
}

fn mutate<F>(root: &Path, ids: &[String], change: F) -> Result<(), String>
where
    F: Fn(&mut Record),
{
    with_write(root, |records| {
        for record in records.iter_mut() {
            if ids.iter().any(|id| id == &record.id) {
                change(record);
            }
        }
        Ok(())
    })
}

pub fn set_favorite(root: &Path, id: &str, favorite: bool) -> Result<(), String> {
    mutate(root, &[id.to_string()], |record| record.favorite = favorite)
}

pub fn set_tags(root: &Path, id: &str, tags: Vec<String>) -> Result<(), String> {
    mutate(root, &[id.to_string()], |record| record.tags = tags.clone())
}

pub fn rename(root: &Path, id: &str, name: String) -> Result<(), String> {
    mutate(root, &[id.to_string()], |record| record.name = name.clone())
}

pub fn soft_delete(root: &Path, ids: &[String]) -> Result<(), String> {
    let now = chrono::Utc::now().timestamp_millis();
    mutate(root, ids, |record| record.deleted_at = Some(now))
}

pub fn restore(root: &Path, ids: &[String]) -> Result<(), String> {
    mutate(root, ids, |record| record.deleted_at = None)
}

pub fn purge(root: &Path) -> Result<(), String> {
    with_write(root, |records| {
        let (gone, kept): (Vec<_>, Vec<_>) = records.iter().cloned().partition(|record| record.deleted_at.is_some());
        for record in gone {
            let _ = std::fs::remove_file(&record.file);
            let _ = std::fs::remove_file(&record.base_file);
            let _ = std::fs::remove_file(&record.document);
        }
        *records = kept;
        Ok(())
    })
}

pub fn export(root: &Path, ids: &[String], dest: &Path) -> Result<u32, String> {
    std::fs::create_dir_all(dest).map_err(err)?;
    let records = with_read(root, |records| Ok(records.to_vec()))?;
    let mut count = 0u32;
    for record in records {
        if !ids.iter().any(|id| id == &record.id) {
            continue;
        }
        let name = sanitize(&record.name);
        let target = dest.join(format!("{name}-{}.png", record.id));
        std::fs::copy(&record.file, target).map_err(err)?;
        count += 1;
    }
    Ok(count)
}

fn sanitize(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|ch| if "\\/:*?\"<>|".contains(ch) { '-' } else { ch })
        .collect();
    let trimmed = cleaned.trim();
    if trimmed.is_empty() { "shotlet".into() } else { trimmed.to_string() }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TempRoot(PathBuf);

    impl TempRoot {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "shotlet-history-{name}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
            ));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TempRoot {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
            if let Ok(mut cache) = index_cache().write() {
                cache.remove(&self.0);
            }
        }
    }

    fn sample() -> Record {
        Record {
            id: "1".into(),
            created_at: 1,
            name: "发票".into(),
            tags: vec!["报销".into()],
            favorite: true,
            width: 10,
            height: 10,
            ocr_text: "合计 128".into(),
            file: String::new(),
            base_file: String::new(),
            document: String::new(),
            deleted_at: None,
        }
    }

    #[test]
    fn search_matches_ocr_name_and_tags() {
        let record = sample();
        assert!(record_matches(&record, "128", "all"));
        assert!(record_matches(&record, "报销", "favorite"));
        assert!(!record_matches(&record, "128", "deleted"));
        let mut deleted = record.clone();
        deleted.deleted_at = Some(2);
        assert!(record_matches(&deleted, "", "deleted"));
        assert!(!record_matches(&deleted, "", "all"));
    }

    #[test]
    fn missing_index_stays_empty() {
        let root = TempRoot::new("missing");
        let items = list(&root.0, "", "all").unwrap();
        assert!(items.is_empty());
        assert!(!root.0.join("index.json").exists());
    }

    #[test]
    fn corrupt_index_is_kept() {
        let root = TempRoot::new("corrupt");
        let path = root.0.join("index.json");
        std::fs::write(&path, "{").unwrap();
        let error = rename(&root.0, "1", "新名字".into()).unwrap_err();
        assert!(error.contains("损坏"), "{error}");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{");
        assert_eq!(std::fs::read_to_string(root.0.join("index.json.broken")).unwrap(), "{");
        assert!(list(&root.0, "", "all").is_err());
    }

    #[test]
    fn readable_index_is_listed_from_memory() {
        let root = TempRoot::new("readable");
        let text = serde_json::to_string(&vec![sample()]).unwrap();
        std::fs::write(root.0.join("index.json"), &text).unwrap();
        let items = list(&root.0, "发票", "all").unwrap();
        assert_eq!(items.len(), 1);
        std::fs::write(root.0.join("index.json"), "[]").unwrap();
        let cached = list(&root.0, "", "all").unwrap();
        assert_eq!(cached.len(), 1, "a later disk change must not bypass the locked cache");
    }
}
