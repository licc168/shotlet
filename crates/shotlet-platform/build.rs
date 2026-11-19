fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").ok().as_deref() == Some("macos") {
        compile_macos_clipboard();
    }
}

fn macos_clang_arch() -> &'static str {
    match std::env::var("CARGO_CFG_TARGET_ARCH").ok().as_deref() {
        Some("x86_64") => "x86_64",
        Some("aarch64") => "arm64",
        other => panic!("unsupported macOS target arch: {other:?}"),
    }
}

fn compile_macos_clipboard() {
    let manifest = std::path::PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let out_dir = std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let arch = macos_clang_arch();
    let source = manifest.join("macos").join("clipboard.m");
    println!("cargo:rerun-if-changed={}", source.display());
    let object = out_dir.join("clipboard.o");
    let status = std::process::Command::new("xcrun")
        .args(["--sdk", "macosx", "clang", "-arch", arch, "-fobjc-arc", "-O2", "-c"])
        .arg(&source)
        .arg("-o")
        .arg(&object)
        .status()
        .unwrap_or_else(|error| panic!("failed to run clang for clipboard.m: {error}"));
    if !status.success() {
        panic!("failed to compile clipboard.m");
    }
    let archive = out_dir.join("libshotlet_platform_mac.a");
    let status = std::process::Command::new("xcrun")
        .args(["--sdk", "macosx", "libtool", "-static", "-arch_only", arch, "-o"])
        .arg(&archive)
        .arg(&object)
        .status()
        .unwrap_or_else(|error| panic!("failed to run libtool for the macOS clipboard: {error}"));
    if !status.success() {
        panic!("failed to archive the macOS clipboard");
    }
    println!("cargo:rustc-link-search=native={}", out_dir.display());
    println!("cargo:rustc-link-lib=static=shotlet_platform_mac");
    println!("cargo:rustc-link-lib=framework=Foundation");
    println!("cargo:rustc-link-lib=framework=AppKit");
    println!("cargo:rustc-link-lib=objc");
}
