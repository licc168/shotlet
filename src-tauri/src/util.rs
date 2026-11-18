use base64::Engine;
use image::codecs::png::{CompressionType, FilterType, PngEncoder};
use image::{ExtendedColorType, ImageEncoder, RgbaImage};
use std::io::BufWriter;
use std::path::Path;

pub fn err(error: impl ToString) -> String {
    error.to_string()
}

pub fn decode_png(data: &str) -> Result<Vec<u8>, String> {
    let raw = data.split(',').next_back().unwrap_or(data).trim();
    base64::engine::general_purpose::STANDARD
        .decode(raw)
        .map_err(err)
}

pub fn load_rgba(bytes: &[u8]) -> Result<RgbaImage, String> {
    let image = image::load_from_memory(bytes).map_err(err)?;
    Ok(image.to_rgba8())
}

pub fn write_png(path: &Path, image: &RgbaImage) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(err)?;
    }
    let file = std::fs::File::create(path).map_err(err)?;
    let writer = BufWriter::new(file);
    PngEncoder::new_with_quality(writer, CompressionType::Fast, FilterType::NoFilter)
        .write_image(image.as_raw(), image.width(), image.height(), ExtendedColorType::Rgba8)
        .map_err(err)
}

pub fn read_png_file(path: &Path) -> Result<RgbaImage, String> {
    let bytes = std::fs::read(path).map_err(err)?;
    load_rgba(&bytes)
}

pub fn copy_text(text: &str) -> Result<(), String> {
    let mut clipboard = arboard::Clipboard::new().map_err(err)?;
    clipboard.set_text(text).map_err(err)
}

pub fn copy_rgba(image: &RgbaImage) -> Result<(), String> {
    let (width, height) = image.dimensions();
    let mut clipboard = arboard::Clipboard::new().map_err(err)?;
    clipboard
        .set_image(arboard::ImageData {
            width: width as usize,
            height: height as usize,
            bytes: std::borrow::Cow::Owned(image.clone().into_raw()),
        })
        .map_err(err)
}

pub fn clipboard_rgba() -> Result<RgbaImage, String> {
    let mut clipboard = arboard::Clipboard::new().map_err(err)?;
    let image = clipboard.get_image().map_err(|_| "剪贴板里没有图像".to_string())?;
    let width = u32::try_from(image.width).map_err(err)?;
    let height = u32::try_from(image.height).map_err(err)?;
    RgbaImage::from_raw(width, height, image.bytes.into_owned()).ok_or_else(|| "剪贴板图像无法读取".into())
}

pub fn new_id() -> String {
    format!("{}", chrono::Utc::now().timestamp_millis())
}
