//! Hand the frozen frame to the overlay without serializing it.
//!
//! ShareX, Flameshot, and ksnip paint the bitmap in the overlay process.
//! Tauri's normal reply turns these pixels into a JSON array and evals it,
//! which is the one-to-two second gap after the frame appears. WebView2 can
//! map the same bytes into the page instead.

use tauri::WebviewWindow;
use webview2_com::Microsoft::Web::WebView2::Win32::{
    COREWEBVIEW2_SHARED_BUFFER_ACCESS_READ_ONLY, ICoreWebView2Environment12, ICoreWebView2_17,
};
use windows::core::Interface;

use crate::capture;

pub fn post(window: &WebviewWindow, path: &str, slot: usize, generation: u64) {
    let Some(image) = capture::preview_image(path) else {
        crate::util::trace("f1: buffer missing");
        return;
    };
    let path = path.to_string();
    let posted = window.with_webview(move |webview| {
        if let Err(error) = send(&webview, image.as_raw(), image.width(), image.height(), &path, slot, generation) {
            eprintln!("preview buffer: {error}");
            crate::util::trace(&format!("f1: buffer failed {error}"));
        }
    });
    if let Err(error) = posted {
        eprintln!("preview buffer: {error}");
    }
}

fn send(
    webview: &tauri::webview::PlatformWebview,
    pixels: &[u8],
    width: u32,
    height: u32,
    path: &str,
    slot: usize,
    generation: u64,
) -> Result<(), String> {
    let environment = webview
        .environment()
        .cast::<ICoreWebView2Environment12>()
        .map_err(|error| error.to_string())?;
    let view = unsafe { webview.controller().CoreWebView2() }
        .map_err(|error| error.to_string())?
        .cast::<ICoreWebView2_17>()
        .map_err(|error| error.to_string())?;
    let buffer = unsafe { environment.CreateSharedBuffer(pixels.len() as u64) }.map_err(|error| error.to_string())?;
    let mut destination = std::ptr::null_mut();
    unsafe { buffer.Buffer(&mut destination) }.map_err(|error| error.to_string())?;
    if destination.is_null() {
        return Err("共享缓冲区是空的".into());
    }
    unsafe { std::ptr::copy_nonoverlapping(pixels.as_ptr(), destination, pixels.len()) };
    let extra = serde_json::json!({
        "slot": slot,
        "generation": generation,
        "path": path,
        "width": width,
        "height": height,
    })
    .to_string();
    let mut wide: Vec<u16> = extra.encode_utf16().collect();
    wide.push(0);
    unsafe {
        view.PostSharedBufferToScript(
            &buffer,
            COREWEBVIEW2_SHARED_BUFFER_ACCESS_READ_ONLY,
            windows::core::PCWSTR(wide.as_ptr()),
        )
    }
    .map_err(|error| error.to_string())?;
    crate::util::trace(&format!("f1: buffer posted slot={slot} bytes={}", pixels.len()));
    Ok(())
}
