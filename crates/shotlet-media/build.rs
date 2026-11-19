fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").ok().as_deref() == Some("macos") {
        compile_macos_helpers();
    }
}

fn macos_clang_arch() -> &'static str {
    match std::env::var("CARGO_CFG_TARGET_ARCH").ok().as_deref() {
        Some("x86_64") => "x86_64",
        Some("aarch64") => "arm64",
        other => panic!("unsupported macOS target arch: {other:?}"),
    }
}

fn compile_macos_helpers() {
    let manifest = std::path::PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let out_dir = std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let arch = macos_clang_arch();
    let mut objects = Vec::new();
    for name in ["ocr_mac.m", "scroll_mac.m"] {
        let source = manifest.join("macos").join(name);
        println!("cargo:rerun-if-changed={}", source.display());
        let object = out_dir.join(name.replace(".m", ".o"));
        let status = std::process::Command::new("xcrun")
            .args(["--sdk", "macosx", "clang", "-arch", arch, "-fobjc-arc", "-O2", "-c"])
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

    let archive = out_dir.join("libshotlet_media_mac.a");
    let mut command = std::process::Command::new("xcrun");
    command
        .args(["--sdk", "macosx", "libtool", "-static", "-arch_only", arch, "-o"])
        .arg(&archive);
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
    println!("cargo:rustc-link-lib=static=shotlet_media_mac");
    println!("cargo:rustc-link-lib=framework=Foundation");
    println!("cargo:rustc-link-lib=framework=AppKit");
    println!("cargo:rustc-link-lib=framework=Vision");
    println!("cargo:rustc-link-lib=framework=CoreGraphics");
    println!("cargo:rustc-link-lib=objc");
}
