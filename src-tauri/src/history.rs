use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use crate::util::{decode_png, err, load_rgba, new_id, write_png};

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Record {
    pub id: String,
    pub created_at: i64,
    pub name: String,
    pub tags: Vec<String>,
    pub favorite: bool,
    pub width: u32,
    pub height: u32,
    pub ocr_text: String,
    pub file: String,
    pub base_file: String,
    pub document: String,
    #[serde(default)]
    pub deleted_at: Option<i64>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryItem {
    pub id: String,
    pub created_at: i64,
    pub name: String,
    pub tags: Vec<String>,
    pub favorite: bool,
    pub width: u32,
    pub height: u32,
    pub ocr_text: String,
    pub file: String,
    pub deleted: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommitRequest {
    pub png_base64: String,
    pub base_png_base64: String,
    pub document: String,
    pub ocr_text: String,
    pub width: u32,
    pub height: u32,
    pub name: String,
}

fn index_path(root: &Path) -> PathBuf {
    root.join("index.json")
}

fn load_all(root: &Path) -> Vec<Record> {
    let Ok(text) = std::fs::read_to_string(index_path(root)) else {
        return Vec::new();
    };
    serde_json::from_str(&text).unwrap_or_default()
}

fn store_all(root: &Path, records: &[Record]) -> Result<(), String> {
    std::fs::create_dir_all(root).map_err(err)?;
    let text = serde_json::to_string_pretty(records).map_err(err)?;
    let path = index_path(root);
    let tmp = root.join("index.json.tmp");
    std::fs::write(&tmp, text).map_err(err)?;
    std::fs::rename(&tmp, path).map_err(err)
}

pub fn list(root: &Path, query: &str, filter: &str) -> Vec<HistoryItem> {
    let needle = query.trim().to_lowercase();
    load_all(root)
        .into_iter()
        .filter(|record| record_matches(record, &needle, filter))
        .map(to_item)
        .collect()
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
    let id = new_id();
    let images = root.join("images");
    let docs = root.join("docs");
    std::fs::create_dir_all(&images).map_err(err)?;
    std::fs::create_dir_all(&docs).map_err(err)?;
    let file = images.join(format!("{id}.png"));
    let base_file = images.join(format!("{id}.base.png"));
    let doc_file = docs.join(format!("{id}.json"));
    write_png(&file, &load_rgba(&decode_png(&request.png_base64)?)?)?;
    write_png(&base_file, &load_rgba(&decode_png(&request.base_png_base64)?)?)?;
    std::fs::write(&doc_file, &request.document).map_err(err)?;
    let name = if request.name.trim().is_empty() {
        format!("截图 {id}")
    } else {
        request.name
    };
    let record = Record {
        id,
        created_at: chrono::Utc::now().timestamp_millis(),
        name,
        tags: Vec::new(),
        favorite: false,
        width: request.width,
        height: request.height,
        ocr_text: request.ocr_text,
        file: file.to_string_lossy().to_string(),
        base_file: base_file.to_string_lossy().to_string(),
        document: doc_file.to_string_lossy().to_string(),
        deleted_at: None,
    };
    let mut records = load_all(root);
    records.insert(0, record.clone());
    store_all(root, &records)?;
    Ok(to_item(record))
}

pub fn commit_image(root: &Path, image: &image::RgbaImage, name: &str) -> Result<HistoryItem, String> {
    let id = new_id();
    let images = root.join("images");
    let docs = root.join("docs");
    std::fs::create_dir_all(&images).map_err(err)?;
    std::fs::create_dir_all(&docs).map_err(err)?;
    let file = images.join(format!("{id}.png"));
    let base_file = images.join(format!("{id}.base.png"));
    let doc_file = docs.join(format!("{id}.json"));
    write_png(&file, image)?;
    write_png(&base_file, image)?;
    std::fs::write(&doc_file, "[]").map_err(err)?;
    let label = if name.trim().is_empty() {
        format!("截图 {id}")
    } else {
        name.to_string()
    };
    let record = Record {
        id,
        created_at: chrono::Utc::now().timestamp_millis(),
        name: label,
        tags: Vec::new(),
        favorite: false,
        width: image.width(),
        height: image.height(),
        ocr_text: String::new(),
        file: file.to_string_lossy().to_string(),
        base_file: base_file.to_string_lossy().to_string(),
        document: doc_file.to_string_lossy().to_string(),
        deleted_at: None,
    };
    let mut records = load_all(root);
    records.insert(0, record.clone());
    store_all(root, &records)?;
    Ok(to_item(record))
}

pub fn update_document(root: &Path, id: &str, request: CommitRequest) -> Result<HistoryItem, String> {
    let mut records = load_all(root);
    let Some(record) = records.iter_mut().find(|record| record.id == id) else {
        return Err("找不到这条历史记录".into());
    };
    write_png(Path::new(&record.file), &load_rgba(&decode_png(&request.png_base64)?)?)?;
    write_png(Path::new(&record.base_file), &load_rgba(&decode_png(&request.base_png_base64)?)?)?;
    std::fs::write(&record.document, &request.document).map_err(err)?;
    record.width = request.width;
    record.height = request.height;
    record.ocr_text = request.ocr_text;
    if !request.name.trim().is_empty() {
        record.name = request.name;
    }
    let item = to_item(record.clone());
    store_all(root, &records)?;
    Ok(item)
}

pub fn editor_source(root: &Path, id: &str) -> Result<(String, u32, u32, String), String> {
    let records = load_all(root);
    let Some(record) = records.into_iter().find(|record| record.id == id) else {
        return Err("找不到这条历史记录".into());
    };
    let document = std::fs::read_to_string(&record.document).unwrap_or_else(|_| "[]".into());
    Ok((record.base_file, record.width, record.height, document))
}

pub fn latest_file(root: &Path) -> Option<String> {
    load_all(root)
        .into_iter()
        .find(|record| record.deleted_at.is_none())
        .map(|record| record.file)
}

fn mutate<F>(root: &Path, ids: &[String], change: F) -> Result<(), String>
where
    F: Fn(&mut Record),
{
    let mut records = load_all(root);
    for record in &mut records {
        if ids.iter().any(|id| id == &record.id) {
            change(record);
        }
    }
    store_all(root, &records)
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
    let records = load_all(root);
    let (gone, kept): (Vec<_>, Vec<_>) = records.into_iter().partition(|record| record.deleted_at.is_some());
    for record in gone {
        let _ = std::fs::remove_file(&record.file);
        let _ = std::fs::remove_file(&record.base_file);
        let _ = std::fs::remove_file(&record.document);
    }
    store_all(root, &kept)
}

pub fn export(root: &Path, ids: &[String], dest: &Path) -> Result<u32, String> {
    std::fs::create_dir_all(dest).map_err(err)?;
    let records = load_all(root);
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

    #[test]
    fn search_matches_ocr_name_and_tags() {
        let record = Record {
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
        };
        assert!(record_matches(&record, "128", "all"));
        assert!(record_matches(&record, "报销", "favorite"));
        assert!(!record_matches(&record, "128", "deleted"));
        let mut deleted = record.clone();
        deleted.deleted_at = Some(2);
        assert!(record_matches(&deleted, "", "deleted"));
        assert!(!record_matches(&deleted, "", "all"));
    }
}
