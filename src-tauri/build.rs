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
    if std::env::var("CARGO_CFG_TARGET_OS").ok().as_deref() == Some("macos") {
        compile_macos_ocr();
    }
    tauri_build::build()
}

fn compile_macos_ocr() {
    let manifest = std::path::PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let out_dir = std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let mut objects = Vec::new();
    for name in ["ocr_mac.m", "overlay_mac.m", "scroll_mac.m"] {
        let source = manifest.join("src").join(name);
        println!("cargo:rerun-if-changed={}", source.display());
        let object = out_dir.join(name.replace(".m", ".o"));
        let status = std::process::Command::new("xcrun")
            .args(["--sdk", "macosx", "clang", "-fobjc-arc", "-O2", "-c"])
            .arg(&source)
            .arg("-o")
            .arg(&object)
            .status()
            .unwrap_or_else(|error| panic!("failed to run clang for {name}: {error}"));
        if !status.success() {
            panic!("failed to compile {name}");
        }
        objects.push(object);
    }

    let archive = out_dir.join("libshotlet_ocr_mac.a");
    let mut command = std::process::Command::new("xcrun");
    command.args(["--sdk", "macosx", "libtool", "-static", "-o"]).arg(&archive);
    for object in &objects {
        command.arg(object);
    }
    let status = command
        .status()
        .unwrap_or_else(|error| panic!("failed to run libtool for macOS helpers: {error}"));
    if !status.success() {
        panic!("failed to archive macOS helpers");
    }

    println!("cargo:rustc-link-search=native={}", out_dir.display());
    println!("cargo:rustc-link-lib=static=shotlet_ocr_mac");
    println!("cargo:rustc-link-lib=framework=Foundation");
    println!("cargo:rustc-link-lib=framework=AppKit");
    println!("cargo:rustc-link-lib=framework=Vision");
    println!("cargo:rustc-link-lib=framework=CoreGraphics");
    println!("cargo:rustc-link-lib=objc");
}
