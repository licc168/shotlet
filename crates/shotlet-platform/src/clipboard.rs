use image::RgbaImage;

use crate::err;

pub fn place(png: &[u8], image: &RgbaImage) -> Result<(), String> {
    #[cfg(windows)]
    {
        place_windows(png, image)
    }
    #[cfg(target_os = "macos")]
    {
        let _ = image;
        place_macos(png)
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        let _ = (png, image);
        Err("当前系统不能写入剪贴板".into())
    }
}

#[cfg(target_os = "macos")]
fn place_macos(png: &[u8]) -> Result<(), String> {
    if !png.starts_with(b"\x89PNG") {
        return Err("剪贴板需要 PNG".into());
    }
    let code = unsafe { shotlet_copy_png(png.as_ptr(), png.len()) };
    if code == 0 {
        Ok(())
    } else {
        Err("无法写入剪贴板".into())
    }
}

#[cfg(target_os = "macos")]
pub fn read_png() -> Option<Vec<u8>> {
    let mut ptr: *mut u8 = std::ptr::null_mut();
    let mut len: usize = 0;
    let code = unsafe { shotlet_read_png(&mut ptr, &mut len) };
    if code != 0 || ptr.is_null() || len == 0 {
        return None;
    }
    let bytes = unsafe { std::slice::from_raw_parts(ptr, len).to_vec() };
    unsafe { shotlet_free(ptr) };
    Some(bytes)
}

#[cfg(target_os = "macos")]
unsafe extern "C" {
    fn shotlet_copy_png(bytes: *const u8, len: usize) -> i32;
    fn shotlet_read_png(out: *mut *mut u8, len: *mut usize) -> i32;
    fn shotlet_free(ptr: *mut u8);
}

#[cfg(windows)]
fn place_windows(png: &[u8], image: &RgbaImage) -> Result<(), String> {
    use windows::core::w;
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::System::DataExchange::{
        EmptyClipboard, OpenClipboard, RegisterClipboardFormatW, SetClipboardData,
    };

    let dib = dib_bottom_up(image)?;
    let png_format = unsafe { RegisterClipboardFormatW(w!("PNG")) };
    unsafe { OpenClipboard(None).map_err(err)? };
    let _close = ClipboardCloser;
    let png_mem = alloc_global(png)?;
    let dib_mem = match alloc_global(&dib) {
        Ok(mem) => mem,
        Err(error) => {
            free_global(png_mem);
            return Err(error);
        }
    };
    if let Err(error) = unsafe { EmptyClipboard().map_err(err) } {
        free_global(png_mem);
        free_global(dib_mem);
        return Err(error);
    }
    let mut stored = false;
    if png_format != 0 && unsafe { SetClipboardData(png_format, Some(HANDLE(png_mem.0))).is_ok() } {
        stored = true;
    } else {
        free_global(png_mem);
    }
    if unsafe { SetClipboardData(8, Some(HANDLE(dib_mem.0))).is_ok() } {
        stored = true;
    } else {
        free_global(dib_mem);
    }
    if stored { Ok(()) } else { Err("无法写入剪贴板".into()) }
}

#[cfg(windows)]
fn alloc_global(bytes: &[u8]) -> Result<windows::Win32::Foundation::HGLOBAL, String> {
    use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};
    let mem = unsafe { GlobalAlloc(GMEM_MOVEABLE, bytes.len()).map_err(err)? };
    let ptr = unsafe { GlobalLock(mem) };
    if ptr.is_null() {
        free_global(mem);
        return Err("剪贴板内存无法锁定".into());
    }
    unsafe {
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), ptr.cast(), bytes.len());
        let _ = GlobalUnlock(mem);
    }
    Ok(mem)
}

#[cfg(windows)]
fn free_global(mem: windows::Win32::Foundation::HGLOBAL) {
    use windows::Win32::Foundation::GlobalFree;
    unsafe {
        let _ = GlobalFree(Some(mem));
    }
}

#[cfg(windows)]
fn dib_bottom_up(image: &RgbaImage) -> Result<Vec<u8>, String> {
    use std::mem::size_of;

    use windows::Win32::Graphics::Gdi::{BI_RGB, BITMAPINFOHEADER};

    let width = image.width() as usize;
    let height = image.height() as usize;
    let row = width.saturating_mul(3).saturating_add(3) & !3;
    let pixels = row.checked_mul(height).ok_or("选区太大")?;
    let header_size = size_of::<BITMAPINFOHEADER>();
    let mut bytes = vec![0u8; header_size.checked_add(pixels).ok_or("选区太大")?];
    let header = BITMAPINFOHEADER {
        biSize: u32::try_from(header_size).map_err(err)?,
        biWidth: i32::try_from(image.width()).map_err(err)?,
        biHeight: i32::try_from(image.height()).map_err(err)?,
        biPlanes: 1,
        biBitCount: 24,
        biCompression: BI_RGB.0,
        biSizeImage: u32::try_from(pixels).unwrap_or(0),
        biXPelsPerMeter: 0,
        biYPelsPerMeter: 0,
        biClrUsed: 0,
        biClrImportant: 0,
    };
    unsafe {
        std::ptr::copy_nonoverlapping(std::ptr::from_ref(&header).cast::<u8>(), bytes.as_mut_ptr(), header_size);
    }
    let raw = image.as_raw();
    for y in 0..height {
        let src_y = height - 1 - y;
        let src_row = src_y.saturating_mul(width);
        let dst_row = header_size + y * row;
        for x in 0..width {
            let src = (src_row + x) * 4;
            let dst = dst_row + x * 3;
            bytes[dst] = raw[src + 2];
            bytes[dst + 1] = raw[src + 1];
            bytes[dst + 2] = raw[src];
        }
    }
    Ok(bytes)
}

#[cfg(windows)]
struct ClipboardCloser;

#[cfg(windows)]
impl Drop for ClipboardCloser {
    fn drop(&mut self) {
        use windows::Win32::System::DataExchange::CloseClipboard;
        unsafe {
            let _ = CloseClipboard();
        }
    }
}
