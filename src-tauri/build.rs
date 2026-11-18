fn main() {
    // include_image! inlines pixels and does not watch the PNG. Stamp it so a
    // new icon rebuilds the crate, and watch the exe icon as well.
    println!("cargo:rerun-if-changed=icons/icon.ico");
    println!("cargo:rerun-if-changed=icons/32x32.png");
    let manifest = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_default();
    let png = std::path::Path::new(&manifest).join("icons/32x32.png");
    let bytes = std::fs::read(&png).unwrap_or_default();
    let mut stamp: u64 = 1469598103934665603;
    for byte in bytes {
        stamp ^= byte as u64;
        stamp = stamp.wrapping_mul(1099511628211);
    }
    let out = std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("tray_icon_stamp.rs");
    std::fs::write(&out, format!("{stamp}u64")).unwrap();
    tauri_build::build()
}
