use std::time::Duration;

use crate::util::{decode_png, err};
use image::RgbaImage;

pub fn recognize_png_base64(data: &str) -> Result<String, String> {
    let bytes = decode_png(data)?;
    recognize_png_bytes(&bytes)
}

pub fn recognize_png_bytes(bytes: &[u8]) -> Result<String, String> {
    #[cfg(windows)]
    {
        // The capture window thread is STA. Waiting on WinRT OCR there never
        // returns, because the completion callback is posted back to that thread.
        let bytes = bytes.to_vec();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::Builder::new()
            .name("shotlet-ocr".into())
            .spawn(move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    let _guard = MtaGuard::enter()?;
                    recognize_windows(&bytes)
                }));
                let result = match result {
                    Ok(result) => result,
                    Err(_) => Err("文字识别中断".into()),
                };
                let _ = tx.send(result);
            })
            .map_err(err)?;
        match rx.recv_timeout(Duration::from_secs(30)) {
            Ok(result) => result,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => Err("文字识别超时".into()),
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => Err("文字识别中断".into()),
        }
    }
    #[cfg(target_os = "macos")]
    {
        let bytes = bytes.to_vec();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::Builder::new()
            .name("shotlet-ocr".into())
            .spawn(move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| recognize_macos(&bytes)));
                let result = match result {
                    Ok(result) => result,
                    Err(_) => Err("文字识别中断".into()),
                };
                let _ = tx.send(result);
            })
            .map_err(err)?;
        match rx.recv_timeout(Duration::from_secs(30)) {
            Ok(result) => result,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => Err("文字识别超时".into()),
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => Err("文字识别中断".into()),
        }
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        let _ = bytes;
        Err("这个版本的 OCR 只在 Windows 和 macOS 上使用系统识别引擎".into())
    }
}

fn fit_within(image: RgbaImage, max_side: u32) -> RgbaImage {
    let max_side = max_side.max(1);
    let (width, height) = image.dimensions();
    if width <= max_side && height <= max_side {
        return image;
    }
    let scale = max_side as f32 / width.max(height) as f32;
    let width = ((width as f32) * scale).floor().max(1.0) as u32;
    let height = ((height as f32) * scale).floor().max(1.0) as u32;
    image::imageops::resize(&image, width, height, image::imageops::FilterType::Triangle)
}

#[cfg(windows)]
struct MtaGuard;

#[cfg(windows)]
impl MtaGuard {
    fn enter() -> Result<Self, String> {
        unsafe {
            let hr = windows::Win32::System::Com::CoInitializeEx(
                None,
                windows::Win32::System::Com::COINIT_MULTITHREADED,
            );
            if hr.is_err() {
                return Err(format!("文字识别无法启动: {hr}"));
            }
        }
        Ok(Self)
    }
}

#[cfg(windows)]
impl Drop for MtaGuard {
    fn drop(&mut self) {
        unsafe { windows::Win32::System::Com::CoUninitialize() }
    }
}

#[cfg(windows)]
fn recognize_windows(png: &[u8]) -> Result<String, String> {
    use windows::Graphics::Imaging::{BitmapAlphaMode, BitmapPixelFormat, SoftwareBitmap};
    use windows::Media::Ocr::OcrEngine;
    use windows::Storage::Streams::DataWriter;

    let image = crate::util::load_rgba(png)?;
    let max_side = OcrEngine::MaxImageDimension().map_err(err)?.max(1);
    let image = fit_within(image, max_side);
    let (width, height) = image.dimensions();
    if width == 0 || height == 0 {
        return Ok(String::new());
    }

    let mut bgra = Vec::with_capacity((width as usize) * (height as usize) * 4);
    for pixel in image.pixels() {
        let [red, green, blue, _] = pixel.0;
        bgra.extend_from_slice(&[blue, green, red, 255]);
    }
    // StoreAsync would flush the bytes into a stream and leave this buffer empty.
    let writer = DataWriter::new().map_err(err)?;
    writer.WriteBytes(&bgra).map_err(err)?;
    let buffer = writer.DetachBuffer().map_err(err)?;
    let bitmap = SoftwareBitmap::CreateCopyWithAlphaFromBuffer(
        &buffer,
        BitmapPixelFormat::Bgra8,
        width as i32,
        height as i32,
        BitmapAlphaMode::Premultiplied,
    )
    .map_err(err)?;
    let engine = ocr_engine()?;
    let result = engine.RecognizeAsync(&bitmap).map_err(err)?.join().map_err(err)?;
    let text = result.Text().map_err(err)?.to_string();
    let text = text.trim().to_string();
    if !text.is_empty() {
        return Ok(text);
    }
    let lines = result.Lines().map_err(err)?;
    let count = lines.Size().map_err(err)?;
    let mut parts = Vec::new();
    for index in 0..count {
        let line = lines.GetAt(index).map_err(err)?.Text().map_err(err)?.to_string();
        let line = line.trim().to_string();
        if !line.is_empty() {
            parts.push(line);
        }
    }
    Ok(parts.join("\n"))
}

#[cfg(windows)]
fn ocr_engine() -> Result<windows::Media::Ocr::OcrEngine, String> {
    use windows::Globalization::Language;
    use windows::Media::Ocr::OcrEngine;

    if let Ok(engine) = OcrEngine::TryCreateFromUserProfileLanguages() {
        return Ok(engine);
    }
    for tag in ["zh-Hans-CN", "zh-Hans", "zh-CN", "zh-Hant-TW", "zh-Hant", "en-US", "en-GB", "ja", "ko-KR"] {
        let language = Language::CreateLanguage(&windows::core::HSTRING::from(tag)).map_err(err)?;
        if OcrEngine::IsLanguageSupported(&language).unwrap_or(false) {
            if let Ok(engine) = OcrEngine::TryCreateFromLanguage(&language) {
                return Ok(engine);
            }
        }
    }
    let languages = OcrEngine::AvailableRecognizerLanguages().map_err(err)?;
    let count = languages.Size().map_err(err)?;
    if count == 0 {
        return Err("系统没有可用的文字识别语言。请在 Windows 设置的“时间和语言”里，为当前语言安装光学字符识别。".into());
    }
    let language = languages.GetAt(0).map_err(err)?;
    OcrEngine::TryCreateFromLanguage(&language).map_err(err)
}


#[cfg(target_os = "macos")]
use std::ffi::{CStr, c_char};

#[cfg(target_os = "macos")]
unsafe extern "C" {
    fn shotlet_recognize_rgba(
        pixels: *const u8,
        width: u32,
        height: u32,
        bytes_per_row: u32,
        error_out: *mut *mut c_char,
    ) -> *mut c_char;
    fn shotlet_ocr_free(ptr: *mut c_char);
}

#[cfg(target_os = "macos")]
fn recognize_macos(png: &[u8]) -> Result<String, String> {
    let image = crate::util::load_rgba(png)?;
    let image = fit_within(image, 4096);
    let (width, height) = image.dimensions();
    if width == 0 || height == 0 {
        return Ok(String::new());
    }
    let bytes_per_row = width.checked_mul(4).ok_or("图片太大，无法识别")?;
    let mut pixels = image.into_raw();
    for alpha in pixels.iter_mut().skip(3).step_by(4) {
        *alpha = 255;
    }

    struct OwnedC(*mut c_char);
    impl Drop for OwnedC {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe { shotlet_ocr_free(self.0) }
            }
        }
    }

    let mut error = std::ptr::null_mut();
    let text = unsafe {
        shotlet_recognize_rgba(pixels.as_ptr(), width, height, bytes_per_row, &mut error)
    };
    let error = OwnedC(error);
    let text = OwnedC(text);
    if !error.0.is_null() {
        let message = unsafe { CStr::from_ptr(error.0) }.to_string_lossy();
        return Err(if message.is_empty() {
            "文字识别失败".into()
        } else {
            message.into_owned()
        });
    }
    if text.0.is_null() {
        return Err("文字识别失败".into());
    }
    Ok(unsafe { CStr::from_ptr(text.0) }.to_string_lossy().trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::fit_within;
    use image::{Rgba, RgbaImage};

    #[test]
    fn fit_within_shrinks_only_the_long_side() {
        let image = RgbaImage::from_pixel(100, 40, Rgba([9, 8, 7, 255]));
        let fitted = fit_within(image, 50);
        assert_eq!(fitted.dimensions(), (50, 20));

        let image = RgbaImage::from_pixel(20, 10, Rgba([1, 2, 3, 4]));
        let fitted = fit_within(image, 50);
        assert_eq!(fitted.dimensions(), (20, 10));
        assert_eq!(fitted.get_pixel(0, 0).0, [1, 2, 3, 4]);
    }

    #[cfg(windows)]
    #[test]
    fn recognizes_drawn_text() {
        let path = std::env::temp_dir().join("shotlet-ocr-sample.png");
        let path_text = path.to_string_lossy().replace('\'', "''");
        let script = format!(
            r#"
Add-Type -AssemblyName System.Drawing
$bmp = New-Object System.Drawing.Bitmap 980, 280
$g = [System.Drawing.Graphics]::FromImage($bmp)
$g.Clear([System.Drawing.Color]::White)
$g.TextRenderingHint = [System.Drawing.Text.TextRenderingHint]::AntiAliasGridFit
$font = New-Object System.Drawing.Font "Microsoft YaHei", 48
$g.DrawString("Hello OCR 128", $font, [System.Drawing.Brushes]::Black, 36, 40)
$g.DrawString("识别测试", $font, [System.Drawing.Brushes]::Black, 36, 140)
$bmp.Save('{path_text}', [System.Drawing.Imaging.ImageFormat]::Png)
$g.Dispose(); $bmp.Dispose()
"#
        );
        let output = std::process::Command::new("powershell")
            .args(["-NoProfile", "-NonInteractive", "-Command", &script])
            .output()
            .expect("powershell");
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        let bytes = std::fs::read(&path).expect("sample png");
        let text = super::recognize_png_bytes(&bytes).expect("ocr");
        let lower = text.to_lowercase();
        assert!(
            lower.contains("hello") || lower.contains("ocr") || text.contains("128") || text.contains("识别") || text.contains("测试"),
            "unexpected ocr text: {text:?}"
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn recognizes_drawn_text() {
        let path = std::env::temp_dir().join("shotlet-ocr-sample.png");
        let script_path = std::env::temp_dir().join("shotlet-ocr-sample.swift");
        let script = r#"
import AppKit

let width = 1200
let height = 420
guard let rep = NSBitmapImageRep(
    bitmapDataPlanes: nil,
    pixelsWide: width,
    pixelsHigh: height,
    bitsPerSample: 8,
    samplesPerPixel: 4,
    hasAlpha: true,
    isPlanar: false,
    colorSpaceName: .deviceRGB,
    bytesPerRow: 0,
    bitsPerPixel: 0
) else {
    fputs("bitmap failed\n", stderr)
    exit(1)
}
NSGraphicsContext.saveGraphicsState()
NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: rep)
NSColor.white.setFill()
NSRect(x: 0, y: 0, width: width, height: height).fill()
let font = NSFont(name: "PingFangSC-Regular", size: 64) ?? NSFont.systemFont(ofSize: 64)
let attrs: [NSAttributedString.Key: Any] = [
    .font: font,
    .foregroundColor: NSColor.black,
]
("Hello OCR 128" as NSString).draw(at: NSPoint(x: 48, y: 230), withAttributes: attrs)
("识别测试" as NSString).draw(at: NSPoint(x: 48, y: 90), withAttributes: attrs)
NSGraphicsContext.restoreGraphicsState()
guard let png = rep.representation(using: .png, properties: [:]) else {
    fputs("png failed\n", stderr)
    exit(1)
}
do {
    try png.write(to: URL(fileURLWithPath: CommandLine.arguments[1]))
} catch {
    fputs("write failed\n", stderr)
    exit(1)
}
"#;
        std::fs::write(&script_path, script).expect("swift script");
        let output = std::process::Command::new("xcrun")
            .args(["--sdk", "macosx", "swift", script_path.to_str().expect("swift path"), path.to_str().expect("png path")])
            .output()
            .expect("swift");
        assert!(
            output.status.success(),
            "stdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let bytes = std::fs::read(&path).expect("sample png");
        let text = super::recognize_png_bytes(&bytes).expect("ocr");
        let lower = text.to_lowercase();
        assert!(
            lower.contains("hello") || lower.contains("ocr") || text.contains("128") || text.contains("识别") || text.contains("测试"),
            "unexpected ocr text: {text:?}"
        );
    }
}
